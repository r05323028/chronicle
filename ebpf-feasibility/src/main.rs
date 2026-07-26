#![no_std]
#![no_main]

use aya_ebpf::{
    EbpfContext,
    bindings::{
        BPF_SOCK_OPS_ACTIVE_ESTABLISHED_CB, BPF_SOCK_OPS_PASSIVE_ESTABLISHED_CB,
        BPF_SOCK_OPS_STATE_CB, BPF_SOCK_OPS_STATE_CB_FLAG,
    },
    helpers::{bpf_get_current_pid_tgid, generated},
    macros::{cgroup_skb, cgroup_sock_addr, map, sock_ops},
    maps::{PerCpuArray, RingBuf},
    programs::{SkBuffContext, SockAddrContext, SockOpsContext},
};

const EVENT_CONNECT4: u8 = 1;
const EVENT_CONNECT6: u8 = 2;
const EVENT_SOCKOPS: u8 = 3;
const EVENT_INGRESS: u8 = 4;
const EVENT_EGRESS: u8 = 5;
const FLAG_TRUNCATED: u32 = 1;
const FLAG_LOAD_ERROR: u32 = 2;
const PAYLOAD_BYTES: usize = 256;
const CONTINUATION_BYTES: usize = 16 * 1024;
const CONTINUATION_MAGIC: u32 = 0x4348_4331;
const CONTINUATION_HEADER_BYTES: usize = 48;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ProbeEvent {
    pub timestamp_ns: u64,
    pub cgroup_id: u64,
    pub socket_cookie: u64,
    pub pid_tgid: u64,
    pub local_addr: [u8; 16],
    pub remote_addr: [u8; 16],
    pub tcp_sequence: u32,
    pub observed_len: u32,
    pub captured_len: u32,
    pub gso_size: u32,
    pub gso_segs: u32,
    pub skb_len: u32,
    pub linear_len: u32,
    pub flags: u32,
    pub local_port: u16,
    pub remote_port: u16,
    pub kind: u8,
    pub family: u8,
    pub direction: u8,
    pub state: u8,
    pub payload: [u8; PAYLOAD_BYTES],
}

impl ProbeEvent {
    const fn empty(kind: u8) -> Self {
        Self {
            timestamp_ns: 0,
            cgroup_id: 0,
            socket_cookie: 0,
            pid_tgid: 0,
            local_addr: [0; 16],
            remote_addr: [0; 16],
            tcp_sequence: 0,
            observed_len: 0,
            captured_len: 0,
            gso_size: 0,
            gso_segs: 0,
            skb_len: 0,
            linear_len: 0,
            flags: 0,
            local_port: 0,
            remote_port: 0,
            kind,
            family: 0,
            direction: 0,
            state: 0,
            payload: [0; PAYLOAD_BYTES],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ContinuationHeader {
    magic: u32,
    header_bytes: u16,
    family: u8,
    direction: u8,
    timestamp_ns: u64,
    socket_cookie: u64,
    cgroup_id: u64,
    tcp_sequence: u32,
    observed_len: u32,
    payload_offset: u32,
    payload_len: u32,
}

#[map]
static EVENTS: RingBuf = RingBuf::with_byte_size(8 * 1024 * 1024, 0);

#[map]
static COUNTERS: PerCpuArray<u64> = PerCpuArray::with_max_entries(2, 0);

#[cgroup_sock_addr(connect4)]
pub fn connect4(ctx: SockAddrContext) -> i32 {
    emit_connect(&ctx, EVENT_CONNECT4);
    1
}

#[cgroup_sock_addr(connect6)]
pub fn connect6(ctx: SockAddrContext) -> i32 {
    emit_connect(&ctx, EVENT_CONNECT6);
    1
}

fn emit_connect(ctx: &SockAddrContext, kind: u8) {
    let mut event = common_event(kind, ctx.as_ptr(), true);
    unsafe {
        let address = &*ctx.sock_addr;
        event.family = address.family as u8;
        event.remote_port = u16::from_be(address.user_port as u16);
        if kind == EVENT_CONNECT4 {
            event.remote_addr[12..16].copy_from_slice(&address.user_ip4.to_ne_bytes());
        } else {
            let words = address.user_ip6;
            let mut index = 0;
            while index < 4 {
                let bytes = words[index].to_ne_bytes();
                event.remote_addr[index * 4..index * 4 + 4].copy_from_slice(&bytes);
                index += 1;
            }
        }
    }
    emit(&event);
}

#[sock_ops]
pub fn socket_lifecycle(ctx: SockOpsContext) -> u32 {
    let op = ctx.op();
    if op == BPF_SOCK_OPS_ACTIVE_ESTABLISHED_CB || op == BPF_SOCK_OPS_PASSIVE_ESTABLISHED_CB {
        let _ = ctx.set_cb_flags(BPF_SOCK_OPS_STATE_CB_FLAG as i32);
    }
    if op == BPF_SOCK_OPS_ACTIVE_ESTABLISHED_CB
        || op == BPF_SOCK_OPS_PASSIVE_ESTABLISHED_CB
        || op == BPF_SOCK_OPS_STATE_CB
    {
        let mut event = common_event(EVENT_SOCKOPS, ctx.as_ptr(), false);
        event.family = ctx.family() as u8;
        event.local_port = ctx.local_port() as u16;
        event.remote_port = u16::from_be(ctx.remote_port() as u16);
        if op == BPF_SOCK_OPS_STATE_CB {
            event.direction = 3;
            event.state = ctx.arg(1) as u8;
            event.flags |= (ctx.arg(0) & 0xff) << 8;
        } else {
            event.direction = if op == BPF_SOCK_OPS_ACTIVE_ESTABLISHED_CB {
                1
            } else {
                2
            };
            event.state = 1;
        }
        if event.family == 2 {
            event.local_addr[12..16].copy_from_slice(&ctx.local_ip4().to_ne_bytes());
            event.remote_addr[12..16].copy_from_slice(&ctx.remote_ip4().to_ne_bytes());
        }
        emit(&event);
    }
    0
}

#[cgroup_skb]
pub fn ingress(ctx: SkBuffContext) -> i32 {
    emit_skb(&ctx, EVENT_INGRESS, 1);
    1
}

#[cgroup_skb]
pub fn egress(ctx: SkBuffContext) -> i32 {
    emit_skb(&ctx, EVENT_EGRESS, 2);
    1
}

fn emit_skb(ctx: &SkBuffContext, kind: u8, direction: u8) {
    let mut event = common_event(kind, ctx.as_ptr(), false);
    event.direction = direction;
    unsafe {
        event.gso_size = (*ctx.skb.skb).gso_size;
        event.gso_segs = (*ctx.skb.skb).gso_segs;
        event.skb_len = (*ctx.skb.skb).len;
        event.linear_len = (*ctx.skb.skb).data_end.saturating_sub((*ctx.skb.skb).data);
    }
    if ctx.len() == 0 {
        return;
    }
    let mut first = [0_u8; 1];
    if !skb_load::<1>(ctx, 0, first.as_mut_ptr()) {
        event.flags |= FLAG_LOAD_ERROR;
        emit(&event);
        return;
    }
    let (tcp_offset, family) = match first[0] >> 4 {
        4 => {
            let mut ip = [0_u8; 20];
            if !skb_load::<20>(ctx, 0, ip.as_mut_ptr()) || ip[9] != 6 {
                return;
            }
            event.local_addr[12..16].copy_from_slice(&ip[12..16]);
            event.remote_addr[12..16].copy_from_slice(&ip[16..20]);
            (usize::from(ip[0] & 0x0f) * 4, 2)
        }
        6 => {
            let mut ip = [0_u8; 40];
            if !skb_load::<40>(ctx, 0, ip.as_mut_ptr()) || ip[6] != 6 {
                return;
            }
            event.local_addr.copy_from_slice(&ip[8..24]);
            event.remote_addr.copy_from_slice(&ip[24..40]);
            (40, 10)
        }
        _ => return,
    };
    event.family = family;
    let mut tcp = [0_u8; 20];
    if !skb_load::<20>(ctx, tcp_offset, tcp.as_mut_ptr()) {
        event.flags |= FLAG_LOAD_ERROR;
        emit(&event);
        return;
    }
    event.local_port = u16::from_be_bytes([tcp[0], tcp[1]]);
    event.remote_port = u16::from_be_bytes([tcp[2], tcp[3]]);
    event.tcp_sequence = u32::from_be_bytes([tcp[4], tcp[5], tcp[6], tcp[7]]);
    let payload_offset = tcp_offset + usize::from(tcp[12] >> 4) * 4;
    let total_len = ctx.len() as usize;
    if payload_offset >= total_len {
        emit(&event);
        return;
    }
    let observed_len = total_len - payload_offset;
    event.observed_len = observed_len as u32;
    emit_continuations(ctx, payload_offset, observed_len, &event);
    let captured_len = if observed_len >= 256 {
        load_payload::<256>(ctx, payload_offset, &mut event.payload)
    } else if observed_len >= 128 {
        load_payload::<128>(ctx, payload_offset, &mut event.payload)
    } else if observed_len >= 64 {
        load_payload::<64>(ctx, payload_offset, &mut event.payload)
    } else if observed_len >= 32 {
        load_payload::<32>(ctx, payload_offset, &mut event.payload)
    } else if observed_len >= 16 {
        load_payload::<16>(ctx, payload_offset, &mut event.payload)
    } else if observed_len >= 8 {
        load_payload::<8>(ctx, payload_offset, &mut event.payload)
    } else if observed_len >= 4 {
        load_payload::<4>(ctx, payload_offset, &mut event.payload)
    } else {
        load_payload::<1>(ctx, payload_offset, &mut event.payload)
    };
    match captured_len {
        Ok(length) => {
            event.captured_len = length as u32;
            if observed_len > length {
                event.flags |= FLAG_TRUNCATED;
            }
        }
        Err(()) => event.flags |= FLAG_LOAD_ERROR,
    }
    emit(&event);
}

#[inline(always)]
fn emit_continuations(
    ctx: &SkBuffContext,
    payload_offset: usize,
    observed_len: usize,
    event: &ProbeEvent,
) {
    if observed_len >= CONTINUATION_BYTES {
        emit_continuation(ctx, payload_offset, 0, event);
    }
    if observed_len >= CONTINUATION_BYTES * 2 {
        emit_continuation(ctx, payload_offset + CONTINUATION_BYTES, 1, event);
    }
    if observed_len >= CONTINUATION_BYTES * 3 {
        emit_continuation(ctx, payload_offset + CONTINUATION_BYTES * 2, 2, event);
    }
    if observed_len >= CONTINUATION_BYTES * 4 {
        emit_continuation(ctx, payload_offset + CONTINUATION_BYTES * 3, 3, event);
    }
}

#[inline(always)]
fn emit_continuation(
    ctx: &SkBuffContext,
    skb_offset: usize,
    continuation_index: u32,
    event: &ProbeEvent,
) {
    let Some(mut entry) = EVENTS.reserve_bytes(CONTINUATION_HEADER_BYTES + CONTINUATION_BYTES, 0)
    else {
        increment_counter(1);
        return;
    };
    let header = ContinuationHeader {
        magic: CONTINUATION_MAGIC,
        header_bytes: CONTINUATION_HEADER_BYTES as u16,
        family: event.family,
        direction: event.direction,
        timestamp_ns: event.timestamp_ns,
        socket_cookie: event.socket_cookie,
        cgroup_id: event.cgroup_id,
        tcp_sequence: event.tcp_sequence,
        observed_len: event.observed_len,
        payload_offset: continuation_index * CONTINUATION_BYTES as u32,
        payload_len: CONTINUATION_BYTES as u32,
    };
    unsafe {
        entry
            .as_mut_ptr()
            .cast::<ContinuationHeader>()
            .write_unaligned(header);
    }
    let destination = unsafe { entry.as_mut_ptr().add(CONTINUATION_HEADER_BYTES) };
    if skb_load::<CONTINUATION_BYTES>(ctx, skb_offset, destination) {
        entry.submit(0);
        increment_counter(0);
    } else {
        entry.discard(0);
    }
}

#[inline(always)]
fn load_payload<const LENGTH: usize>(
    ctx: &SkBuffContext,
    offset: usize,
    payload: &mut [u8; PAYLOAD_BYTES],
) -> Result<usize, ()> {
    if skb_load::<LENGTH>(ctx, offset, payload.as_mut_ptr()) {
        Ok(LENGTH)
    } else {
        Err(())
    }
}

#[inline(always)]
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

fn common_event(kind: u8, context: *mut core::ffi::c_void, include_process: bool) -> ProbeEvent {
    let mut event = ProbeEvent::empty(kind);
    event.timestamp_ns = unsafe { generated::bpf_ktime_get_ns() };
    event.cgroup_id = unsafe { generated::bpf_get_current_cgroup_id() };
    event.socket_cookie = unsafe { generated::bpf_get_socket_cookie(context) };
    if include_process {
        event.pid_tgid = bpf_get_current_pid_tgid();
    }
    event
}

fn emit(event: &ProbeEvent) {
    if EVENTS.output::<ProbeEvent>(event, 0).is_err() {
        increment_counter(1);
    } else {
        increment_counter(0);
    }
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
