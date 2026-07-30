#![no_std]
#![no_main]

//! Private ABI v1 producer. This package emits evidence only; it never parses HTTP
//! or persists packets.

use aya_ebpf::{
    EbpfContext,
    bindings::{
        BPF_SOCK_OPS_ACTIVE_ESTABLISHED_CB, BPF_SOCK_OPS_PASSIVE_ESTABLISHED_CB,
        BPF_SOCK_OPS_STATE_CB, BPF_SOCK_OPS_STATE_CB_FLAG, bpf_sock_addr,
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
const MAX_PAYLOAD_BYTES: usize = 16 * 1024;
const MAX_PAYLOAD_CONTINUATIONS: usize = 4;
const MAX_OBSERVED_PAYLOAD_BYTES: usize = MAX_PAYLOAD_BYTES * MAX_PAYLOAD_CONTINUATIONS;
const PAYLOAD_ITEM_BYTES: usize =
    HEADER_BYTES + COMMON_SOCKET_BYTES + PAYLOAD_HEADER_BYTES + MAX_PAYLOAD_BYTES;
const MAX_SMALL_ITEM_BYTES: usize = HEADER_BYTES + COMMON_SOCKET_BYTES + 38 + 5;

const CONNECT4: u8 = 1;
const CONNECT6: u8 = 2;
const SOCKOPS: u8 = 3;
const INGRESS: u8 = 4;
const EGRESS: u8 = 5;

const SIGNAL_ACTIVE_ESTABLISHED: u8 = 1;
const SIGNAL_AMBIGUOUS: u8 = 4;
const SIGNAL_PASSIVE_ESTABLISHED: u8 = 5;

const ROLE_ACTIVE: u8 = 1;
const ROLE_PASSIVE: u8 = 2;
const AF_INET_FAMILY: u32 = 2;
const AF_INET6_FAMILY: u32 = 10;

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
    let raw = unsafe { &*ctx.as_ptr().cast::<bpf_sock_addr>() };
    emit_connect(
        &ctx,
        CONNECT4,
        u16::from_be(raw.user_port as u16),
        [raw.user_ip4, 0, 0, 0],
    );
    1
}

#[cgroup_sock_addr(connect6)]
pub fn connect6(ctx: SockAddrContext) -> i32 {
    let raw = unsafe { &*ctx.as_ptr().cast::<bpf_sock_addr>() };
    let remote_address = [
        raw.user_ip6[0],
        raw.user_ip6[1],
        raw.user_ip6[2],
        raw.user_ip6[3],
    ];
    emit_connect(
        &ctx,
        CONNECT6,
        u16::from_be(raw.user_port as u16),
        remote_address,
    );
    1
}

fn emit_connect(ctx: &SockAddrContext, kind: u8, port: u16, remote_address: [u32; 4]) {
    if port == 0 {
        return;
    }
    let observed_cgroup_id = unsafe { generated::bpf_get_current_cgroup_id() };
    let mut item = [0_u8; MAX_SMALL_ITEM_BYTES];
    let mut len = write_common(
        &mut item,
        kind,
        ctx.as_ptr(),
        true,
        true,
        observed_cgroup_id,
    );
    item[len] = if kind == CONNECT4 { 4 } else { 6 };
    len += 1;
    if kind == CONNECT4 {
        write_ipv4(&mut item, len, remote_address[0]);
    } else {
        write_ipv6(&mut item, len, remote_address);
    }
    len += 16;
    write_u16(&mut item, len, port);
    len += 2;
    item[len] = ROLE_ACTIVE;
    len += 1;
    write_u32(&mut item, 8, len as u32);
    submit(&item, len);
}

#[sock_ops]
pub fn socket_lifecycle(ctx: SockOpsContext) -> u32 {
    let op = ctx.op();
    let established =
        op == BPF_SOCK_OPS_ACTIVE_ESTABLISHED_CB || op == BPF_SOCK_OPS_PASSIVE_ESTABLISHED_CB;
    if established || op == BPF_SOCK_OPS_STATE_CB {
        let _ = ctx.set_cb_flags(BPF_SOCK_OPS_STATE_CB_FLAG as i32);
    }
    if !established && op != BPF_SOCK_OPS_STATE_CB {
        return 0;
    }
    let state = if op == BPF_SOCK_OPS_STATE_CB {
        ctx.arg(1)
    } else {
        0
    };
    let (family, local_port, remote_port, local_address, remote_address) = if established {
        let raw = unsafe { &*ctx.ops };
        let family = raw.family;
        let local_port = raw.local_port as u16;
        let remote_port = u16::from_be((raw.remote_port >> 16) as u16);
        if local_port == 0 || remote_port == 0 {
            return 0;
        }
        let (local_address, remote_address) = match family {
            AF_INET_FAMILY => ([raw.local_ip4, 0, 0, 0], [raw.remote_ip4, 0, 0, 0]),
            AF_INET6_FAMILY => (
                [
                    raw.local_ip6[0],
                    raw.local_ip6[1],
                    raw.local_ip6[2],
                    raw.local_ip6[3],
                ],
                [
                    raw.remote_ip6[0],
                    raw.remote_ip6[1],
                    raw.remote_ip6[2],
                    raw.remote_ip6[3],
                ],
            ),
            _ => return 0,
        };
        (
            family,
            local_port,
            remote_port,
            local_address,
            remote_address,
        )
    } else {
        (0, 0, 0, [0; 4], [0; 4])
    };
    let mut item = [0_u8; MAX_SMALL_ITEM_BYTES];
    // SockOps may run in peer task context; zero tells userspace to use attachment scope.
    let mut len = write_common(&mut item, SOCKOPS, ctx.as_ptr(), false, established, 0);
    item[len] = match op {
        BPF_SOCK_OPS_ACTIVE_ESTABLISHED_CB => SIGNAL_ACTIVE_ESTABLISHED,
        BPF_SOCK_OPS_PASSIVE_ESTABLISHED_CB => SIGNAL_PASSIVE_ESTABLISHED,
        _ => SIGNAL_AMBIGUOUS,
    };
    len += 1;
    write_u32(&mut item, len, state);
    len += 4;
    if established {
        match family {
            AF_INET_FAMILY => {
                item[len] = 4;
                write_ipv4(&mut item, len + 1, local_address[0]);
                write_ipv4(&mut item, len + 19, remote_address[0]);
            }
            AF_INET6_FAMILY => {
                item[len] = 6;
                write_ipv6(&mut item, len + 1, local_address);
                write_ipv6(&mut item, len + 19, remote_address);
            }
            _ => return 0,
        }
        write_u16(&mut item, len + 17, local_port);
        write_u16(&mut item, len + 35, remote_port);
        item[len + 37] = if op == BPF_SOCK_OPS_ACTIVE_ESTABLISHED_CB {
            ROLE_ACTIVE
        } else {
            ROLE_PASSIVE
        };
        len += 38;
    }
    write_u32(&mut item, 8, len as u32);
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
    let captured_length = core::cmp::min(observed_length, MAX_OBSERVED_PAYLOAD_BYTES);
    let tcp_sequence = u32::from_be_bytes([tcp[4], tcp[5], tcp[6], tcp[7]]);
    let socket_cookie = unsafe { generated::bpf_get_socket_cookie(ctx.as_ptr()) };
    let continuation_position = match unsafe { FIRST_SEQUENCE.get(socket_cookie) } {
        Some(first_sequence) => tcp_sequence.wrapping_sub(*first_sequence),
        None => {
            let _ = FIRST_SEQUENCE.insert(socket_cookie, tcp_sequence, 0);
            0
        }
    };
    if captured_length < MAX_PAYLOAD_BYTES {
        let short_capture = core::cmp::min(observed_length, 64);
        emit_payload_chunk::<64>(
            ctx,
            kind,
            family,
            payload_offset,
            0,
            tcp_sequence,
            continuation_position,
            observed_length,
            short_capture,
        );
        return;
    }
    let mut chunk_offset = 0_usize;
    if captured_length >= MAX_PAYLOAD_BYTES {
        emit_payload_chunk::<MAX_PAYLOAD_BYTES>(
            ctx,
            kind,
            family,
            payload_offset,
            chunk_offset,
            tcp_sequence,
            continuation_position,
            observed_length,
            captured_length,
        );
        chunk_offset += MAX_PAYLOAD_BYTES;
    }
    if captured_length - chunk_offset >= MAX_PAYLOAD_BYTES {
        emit_payload_chunk::<MAX_PAYLOAD_BYTES>(
            ctx,
            kind,
            family,
            payload_offset,
            chunk_offset,
            tcp_sequence,
            continuation_position,
            observed_length,
            captured_length,
        );
        chunk_offset += MAX_PAYLOAD_BYTES;
    }
    if captured_length - chunk_offset >= MAX_PAYLOAD_BYTES {
        emit_payload_chunk::<MAX_PAYLOAD_BYTES>(
            ctx,
            kind,
            family,
            payload_offset,
            chunk_offset,
            tcp_sequence,
            continuation_position,
            observed_length,
            captured_length,
        );
        chunk_offset += MAX_PAYLOAD_BYTES;
    }
    if captured_length > chunk_offset {
        let remaining = captured_length - chunk_offset;
        if remaining >= MAX_PAYLOAD_BYTES {
            emit_payload_chunk::<MAX_PAYLOAD_BYTES>(
                ctx,
                kind,
                family,
                payload_offset,
                chunk_offset,
                tcp_sequence,
                continuation_position,
                observed_length,
                captured_length,
            );
        } else {
            let tail_capture = core::cmp::min(remaining, 64);
            emit_payload_chunk::<64>(
                ctx,
                kind,
                family,
                payload_offset,
                chunk_offset,
                tcp_sequence,
                continuation_position,
                observed_length,
                chunk_offset + tail_capture,
            );
        }
    }
}

fn write_common(
    item: &mut [u8],
    kind: u8,
    context: *mut core::ffi::c_void,
    include_process: bool,
    initialize_identity: bool,
    observed_cgroup_id: u64,
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
    let first_seen = match unsafe { FIRST_SEEN.get(socket_cookie) }.copied() {
        Some(first_seen) => first_seen,
        None if initialize_identity => {
            let _ = FIRST_SEEN.insert(socket_cookie, timestamp, 0);
            timestamp
        }
        None => 0,
    };
    write_u64(item, HEADER_BYTES + 16, first_seen);
    write_u64(item, HEADER_BYTES + 24, 0);
    write_u64(item, HEADER_BYTES + 32, observed_cgroup_id);
    let pid_tgid = if include_process {
        bpf_get_current_pid_tgid()
    } else {
        0
    };
    write_u32(item, HEADER_BYTES + 40, (pid_tgid >> 32) as u32);
    write_u32(item, HEADER_BYTES + 44, pid_tgid as u32);
    size
}

fn submit(item: &[u8], size: usize) {
    if let Some(mut entry) = EVENTS.reserve_bytes(size, 0) {
        unsafe { core::ptr::copy_nonoverlapping(item.as_ptr(), entry.as_mut_ptr(), size) };
        entry.submit(0);
        increment_counter(0);
    } else {
        increment_counter(1);
    }
}

#[inline(always)]
fn emit_payload_chunk<const CHUNK_BYTES: usize>(
    ctx: &SkBuffContext,
    kind: u8,
    family: u8,
    payload_offset: usize,
    chunk_offset: usize,
    tcp_sequence: u32,
    continuation_position: u32,
    observed_length: usize,
    captured_length: usize,
) {
    let remaining = captured_length.saturating_sub(chunk_offset);
    let chunk_length = core::cmp::min(remaining, CHUNK_BYTES);
    if chunk_length == 0 {
        return;
    }
    let size = PAYLOAD_ITEM_BYTES;
    let Some(mut entry) = EVENTS.reserve_bytes(size, 0) else {
        increment_counter(1);
        return;
    };
    let mut header = [0_u8; HEADER_BYTES + COMMON_SOCKET_BYTES + PAYLOAD_HEADER_BYTES];
    let mut len = write_common(&mut header, kind, ctx.as_ptr(), false, false, 0);
    header[len] = family;
    len += 1;
    write_u32(
        &mut header,
        len,
        tcp_sequence.wrapping_add(chunk_offset as u32),
    );
    len += 4;
    write_u32(
        &mut header,
        len,
        continuation_position.wrapping_add(chunk_offset as u32),
    );
    len += 4;
    let is_final_chunk = chunk_offset + chunk_length == captured_length;
    let truncated = observed_length > captured_length && is_final_chunk;
    let chunk_observed_length = if truncated {
        observed_length.saturating_sub(chunk_offset)
    } else {
        chunk_length
    };
    write_u32(&mut header, len, chunk_observed_length as u32);
    len += 4;
    write_u32(&mut header, len, chunk_length as u32);
    len += 4;
    header[len] = u8::from(truncated);
    len += 1;
    write_u32(&mut header, 8, size as u32);
    unsafe {
        core::ptr::copy_nonoverlapping(header.as_ptr(), entry.as_mut_ptr(), len);
    }
    let destination = unsafe { entry.as_mut_ptr().add(len) };
    let loaded = if CHUNK_BYTES == 64 {
        load_small_payload(
            ctx,
            payload_offset + chunk_offset,
            destination,
            chunk_length,
        )
    } else {
        skb_load::<CHUNK_BYTES>(ctx, payload_offset + chunk_offset, destination)
    };
    if !loaded {
        entry.discard(0);
        return;
    }
    entry.submit(0);
    increment_counter(0);
}

#[inline(always)]
fn load_small_payload(
    ctx: &SkBuffContext,
    offset: usize,
    destination: *mut u8,
    length: usize,
) -> bool {
    macro_rules! load_byte {
        ($index:expr) => {
            if length > $index
                && !skb_load::<1>(ctx, offset + $index, unsafe { destination.add($index) })
            {
                return false;
            }
        };
    }
    load_byte!(0);
    load_byte!(1);
    load_byte!(2);
    load_byte!(3);
    load_byte!(4);
    load_byte!(5);
    load_byte!(6);
    load_byte!(7);
    load_byte!(8);
    load_byte!(9);
    load_byte!(10);
    load_byte!(11);
    load_byte!(12);
    load_byte!(13);
    load_byte!(14);
    load_byte!(15);
    load_byte!(16);
    load_byte!(17);
    load_byte!(18);
    load_byte!(19);
    load_byte!(20);
    load_byte!(21);
    load_byte!(22);
    load_byte!(23);
    load_byte!(24);
    load_byte!(25);
    load_byte!(26);
    load_byte!(27);
    load_byte!(28);
    load_byte!(29);
    load_byte!(30);
    load_byte!(31);
    load_byte!(32);
    load_byte!(33);
    load_byte!(34);
    load_byte!(35);
    load_byte!(36);
    load_byte!(37);
    load_byte!(38);
    load_byte!(39);
    load_byte!(40);
    load_byte!(41);
    load_byte!(42);
    load_byte!(43);
    load_byte!(44);
    load_byte!(45);
    load_byte!(46);
    load_byte!(47);
    load_byte!(48);
    load_byte!(49);
    load_byte!(50);
    load_byte!(51);
    load_byte!(52);
    load_byte!(53);
    load_byte!(54);
    load_byte!(55);
    load_byte!(56);
    load_byte!(57);
    load_byte!(58);
    load_byte!(59);
    load_byte!(60);
    load_byte!(61);
    load_byte!(62);
    load_byte!(63);
    true
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

fn write_ipv4(item: &mut [u8], offset: usize, address: u32) {
    item[offset..offset + 4].copy_from_slice(&address.to_ne_bytes());
    item[offset + 4..offset + 16].fill(0);
}

fn write_ipv6(item: &mut [u8], offset: usize, address: [u32; 4]) {
    let mut word = 0;
    while word < 4 {
        let start = offset + word * 4;
        item[start..start + 4].copy_from_slice(&address[word].to_ne_bytes());
        word += 1;
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
