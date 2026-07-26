#![no_std]
#![no_main]

//! Private ABI v1 producer. This package emits evidence only; it never parses HTTP
//! or persists packets.

use aya_ebpf::{
    EbpfContext,
    bindings::{
        BPF_SOCK_OPS_ACTIVE_ESTABLISHED_CB, BPF_SOCK_OPS_STATE_CB, BPF_SOCK_OPS_STATE_CB_FLAG,
    },
    helpers::{bpf_get_current_pid_tgid, generated},
    macros::{cgroup_skb, cgroup_sock_addr, map, sock_ops},
    maps::{HashMap, PerCpuArray, RingBuf},
    programs::{SkBuffContext, SockAddrContext, SockOpsContext},
};

const MAGIC: [u8; 4] = *b"CHRN";
const VERSION: u16 = 1;
const LITTLE_ENDIAN: u8 = 1;
const HEADER_BYTES: usize = 12;
const COMMON_SOCKET_BYTES: usize = 48;
const PAYLOAD_HEADER_BYTES: usize = 18;
const MAX_PAYLOAD_BYTES: usize = 64;
const MAX_ITEM_BYTES: usize =
    HEADER_BYTES + COMMON_SOCKET_BYTES + PAYLOAD_HEADER_BYTES + MAX_PAYLOAD_BYTES;

const CONNECT4: u8 = 1;
const CONNECT6: u8 = 2;
const SOCKOPS: u8 = 3;
const INGRESS: u8 = 4;
const EGRESS: u8 = 5;

const SIGNAL_ACTIVE_ESTABLISHED: u8 = 1;
const SIGNAL_AMBIGUOUS: u8 = 4;

#[map]
static EVENTS: RingBuf = RingBuf::with_byte_size(8 * 1024 * 1024, 0);

#[map]
static COUNTERS: PerCpuArray<u64> = PerCpuArray::with_max_entries(2, 0);

#[map]
static FIRST_SEEN: HashMap<u64, u64> = HashMap::with_max_entries(65_536, 0);

#[map]
static FIRST_SEQUENCE: HashMap<u64, u32> = HashMap::with_max_entries(65_536, 0);

#[cgroup_sock_addr(connect4)]
pub fn connect4(ctx: SockAddrContext) -> i32 {
    emit_connect(&ctx, CONNECT4);
    1
}

#[cgroup_sock_addr(connect6)]
pub fn connect6(ctx: SockAddrContext) -> i32 {
    emit_connect(&ctx, CONNECT6);
    1
}

fn emit_connect(ctx: &SockAddrContext, kind: u8) {
    let mut item = [0_u8; MAX_ITEM_BYTES];
    let len = write_common(&mut item, kind, ctx.as_ptr(), true);
    submit(&item, len);
}

#[sock_ops]
pub fn socket_lifecycle(ctx: SockOpsContext) -> u32 {
    let op = ctx.op();
    if op == BPF_SOCK_OPS_ACTIVE_ESTABLISHED_CB || op == BPF_SOCK_OPS_STATE_CB {
        let _ = ctx.set_cb_flags(BPF_SOCK_OPS_STATE_CB_FLAG as i32);
    }
    if op != BPF_SOCK_OPS_ACTIVE_ESTABLISHED_CB && op != BPF_SOCK_OPS_STATE_CB {
        return 0;
    }
    let mut item = [0_u8; MAX_ITEM_BYTES];
    let mut len = write_common(&mut item, SOCKOPS, ctx.as_ptr(), false);
    item[len] = if op == BPF_SOCK_OPS_ACTIVE_ESTABLISHED_CB {
        SIGNAL_ACTIVE_ESTABLISHED
    } else {
        SIGNAL_AMBIGUOUS
    };
    len += 1;
    let state = if op == BPF_SOCK_OPS_STATE_CB {
        ctx.arg(1)
    } else {
        0
    };
    write_u32(&mut item, len, state);
    len += 4;
    submit(&item, len);
    0
}

#[cgroup_skb]
pub fn ingress(ctx: SkBuffContext) -> i32 {
    emit_skb(&ctx, INGRESS);
    1
}

#[cgroup_skb]
pub fn egress(ctx: SkBuffContext) -> i32 {
    emit_skb(&ctx, EGRESS);
    1
}

fn emit_skb(ctx: &SkBuffContext, kind: u8) {
    if ctx.len() == 0 {
        return;
    }
    let mut first = [0_u8; 1];
    if !skb_load::<1>(ctx, 0, first.as_mut_ptr()) {
        return;
    }
    let (tcp_offset, family) = match first[0] >> 4 {
        4 => {
            let mut ip = [0_u8; 20];
            if !skb_load::<20>(ctx, 0, ip.as_mut_ptr()) || ip[9] != 6 {
                return;
            }
            (usize::from(ip[0] & 0x0f) * 4, 4)
        }
        6 => {
            let mut ip = [0_u8; 40];
            if !skb_load::<40>(ctx, 0, ip.as_mut_ptr()) || ip[6] != 6 {
                return;
            }
            (40, 6)
        }
        _ => return,
    };
    let mut tcp = [0_u8; 20];
    if !skb_load::<20>(ctx, tcp_offset, tcp.as_mut_ptr()) {
        return;
    }
    let payload_offset = tcp_offset + usize::from(tcp[12] >> 4) * 4;
    let observed_length = (ctx.len() as usize).saturating_sub(payload_offset);
    if observed_length == 0 {
        return;
    }
    let captured_length = payload_load_length(observed_length);
    let mut item = [0_u8; MAX_ITEM_BYTES];
    let mut len = write_common(&mut item, kind, ctx.as_ptr(), false);
    item[len] = family;
    len += 1;
    let tcp_sequence = u32::from_be_bytes([tcp[4], tcp[5], tcp[6], tcp[7]]);
    write_u32(&mut item, len, tcp_sequence);
    len += 4;
    let socket_cookie = unsafe { generated::bpf_get_socket_cookie(ctx.as_ptr()) };
    let continuation_position = match unsafe { FIRST_SEQUENCE.get(&socket_cookie) } {
        Some(first_sequence) => tcp_sequence.wrapping_sub(*first_sequence),
        None => {
            let _ = FIRST_SEQUENCE.insert(&socket_cookie, &tcp_sequence, 0);
            0
        }
    };
    write_u32(&mut item, len, continuation_position);
    len += 4;
    write_u32(&mut item, len, observed_length as u32);
    len += 4;
    write_u32(&mut item, len, captured_length as u32);
    len += 4;
    item[len] = u8::from(observed_length != captured_length);
    len += 1;
    if !load_payload(ctx, payload_offset, captured_length, unsafe {
        item.as_mut_ptr().add(len)
    }) {
        return;
    }
    len += captured_length;
    submit(&item, len);
}

fn write_common(
    item: &mut [u8; MAX_ITEM_BYTES],
    kind: u8,
    context: *mut core::ffi::c_void,
    include_process: bool,
) -> usize {
    let size = HEADER_BYTES + COMMON_SOCKET_BYTES;
    item[0..4].copy_from_slice(&MAGIC);
    write_u16(item, 4, VERSION);
    item[6] = kind;
    item[7] = LITTLE_ENDIAN;
    write_u32(item, 8, size as u32);

    let timestamp = unsafe { generated::bpf_ktime_get_ns() };
    write_u64(item, HEADER_BYTES, timestamp);
    let socket_cookie = unsafe { generated::bpf_get_socket_cookie(context) };
    write_u64(item, HEADER_BYTES + 8, socket_cookie);
    let first_seen = if include_process {
        let _ = FIRST_SEEN.insert(&socket_cookie, &timestamp, 0);
        timestamp
    } else {
        unsafe { FIRST_SEEN.get(&socket_cookie) }
            .copied()
            .unwrap_or(0)
    };
    write_u64(item, HEADER_BYTES + 16, first_seen);
    write_u64(item, HEADER_BYTES + 24, 0);
    write_u64(item, HEADER_BYTES + 32, unsafe {
        generated::bpf_get_current_cgroup_id()
    });
    let pid_tgid = if include_process {
        bpf_get_current_pid_tgid()
    } else {
        0
    };
    write_u32(item, HEADER_BYTES + 40, (pid_tgid >> 32) as u32);
    write_u32(item, HEADER_BYTES + 44, pid_tgid as u32);
    size
}

fn submit(item: &[u8; MAX_ITEM_BYTES], size: usize) {
    let mut copy = *item;
    write_u32(&mut copy, 8, size as u32);
    if let Some(mut entry) = EVENTS.reserve_bytes(size, 0) {
        unsafe { core::ptr::copy_nonoverlapping(copy.as_ptr(), entry.as_mut_ptr(), size) };
        entry.submit(0);
        increment_counter(0);
    } else {
        increment_counter(1);
    }
}

const fn payload_load_length(observed_length: usize) -> usize {
    if observed_length >= MAX_PAYLOAD_BYTES {
        MAX_PAYLOAD_BYTES
    } else if observed_length >= 32 {
        32
    } else if observed_length >= 16 {
        16
    } else if observed_length >= 8 {
        8
    } else if observed_length >= 4 {
        4
    } else {
        1
    }
}

fn load_payload(ctx: &SkBuffContext, offset: usize, length: usize, destination: *mut u8) -> bool {
    match length {
        64 => skb_load::<64>(ctx, offset, destination),
        32 => skb_load::<32>(ctx, offset, destination),
        16 => skb_load::<16>(ctx, offset, destination),
        8 => skb_load::<8>(ctx, offset, destination),
        4 => skb_load::<4>(ctx, offset, destination),
        _ => skb_load::<1>(ctx, offset, destination),
    }
}

fn skb_load<const LENGTH: usize>(ctx: &SkBuffContext, offset: usize, destination: *mut u8) -> bool {
    unsafe {
        generated::bpf_skb_load_bytes(
            ctx.as_ptr(),
            offset as u32,
            destination.cast(),
            LENGTH as u32,
        ) == 0
    }
}

fn write_u16(item: &mut [u8], offset: usize, value: u16) {
    item[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn write_u32(item: &mut [u8], offset: usize, value: u32) {
    item[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn write_u64(item: &mut [u8], offset: usize, value: u64) {
    item[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

fn increment_counter(index: u32) {
    if let Some(counter) = COUNTERS.get_ptr_mut(index) {
        unsafe { *counter = (*counter).saturating_add(1) };
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo<'_>) -> ! {
    unsafe { core::hint::unreachable_unchecked() }
}
