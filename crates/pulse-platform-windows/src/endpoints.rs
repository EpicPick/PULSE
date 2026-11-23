use pulse_domain::{Availability, Endpoint, EndpointSnapshot, Transport};
use std::{
    ffi::c_void,
    mem::{offset_of, size_of},
    net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV6},
    sync::Arc,
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::{
            ERROR_ACCESS_DENIED, ERROR_INSUFFICIENT_BUFFER, ERROR_INVALID_DATA,
            ERROR_NOT_SUPPORTED, NO_ERROR,
        },
        NetworkManagement::IpHelper::*,
        Networking::WinSock::{AF_INET, AF_INET6},
    },
    core::HRESULT,
};

const LIMIT: usize = 1024;
const MAX_BYTES: usize = 4 * 1024 * 1024;

#[derive(Default)]
pub struct Endpoints {
    buffer: Vec<u32>,
    cached: Option<(Instant, Arc<EndpointSnapshot>)>,
}

fn status(code: u32) -> Availability<usize> {
    match code {
        c if c == ERROR_ACCESS_DENIED.0 => Availability::PermissionRequired,
        c if c == ERROR_NOT_SUPPORTED.0 => Availability::Unsupported,
        c if c == ERROR_INSUFFICIENT_BUFFER.0 => Availability::Pending,
        c => Availability::Error(HRESULT::from_win32(c).0),
    }
}

fn port(value: u32) -> u16 {
    u16::from_be(value as u16)
}
fn v4(a: u32, p: u32) -> SocketAddr {
    SocketAddr::from((Ipv4Addr::from(a.to_ne_bytes()), port(p)))
}
fn v6(a: [u8; 16], p: u32, scope: u32) -> SocketAddr {
    SocketAddrV6::new(Ipv6Addr::from(a), port(p), 0, scope).into()
}

// SAFETY contract: rows are initialized Win32 repr(C) records with no invalid bit patterns.
unsafe fn parse<T: Copy>(
    bytes: &[u8],
    offset: usize,
    mut read: impl FnMut(T),
) -> Result<(usize, usize), u32> {
    let n = u32::from_ne_bytes(
        bytes
            .get(..4)
            .ok_or(ERROR_INVALID_DATA.0)?
            .try_into()
            .unwrap(),
    ) as usize;
    if n == 0 {
        return Ok((0, 0));
    }
    let end = n
        .checked_mul(size_of::<T>())
        .and_then(|n| offset.checked_add(n))
        .ok_or(ERROR_INVALID_DATA.0)?;
    if end > bytes.len() {
        return Err(ERROR_INVALID_DATA.0);
    }
    let kept = n.min(LIMIT);
    for i in 0..kept {
        // SAFETY: the entire variable-length table was bounds-checked.
        read(unsafe {
            bytes
                .as_ptr()
                .add(offset + i * size_of::<T>())
                .cast::<T>()
                .read_unaligned()
        });
    }
    Ok((kept, n - kept))
}

impl Endpoints {
    fn fetch(&mut self, call: impl Fn(Option<*mut c_void>, &mut u32) -> u32) -> Result<&[u8], u32> {
        for _ in 0..3 {
            let capacity = self.buffer.len() * 4;
            let mut bytes = capacity as u32;
            let code = call(
                (capacity != 0).then(|| self.buffer.as_mut_ptr().cast()),
                &mut bytes,
            );
            let size = bytes as usize;
            if code == NO_ERROR.0 {
                if capacity < 4 {
                    return Err(ERROR_INVALID_DATA.0);
                }
                // SAFETY: the initialized backing allocation exposes capacity bytes.
                return Ok(unsafe {
                    std::slice::from_raw_parts(self.buffer.as_ptr().cast(), size.min(capacity))
                });
            }
            if code != ERROR_INSUFFICIENT_BUFFER.0 || !(4..=MAX_BYTES).contains(&size) {
                return Err(code);
            }
            self.buffer.resize(size.div_ceil(4), 0);
        }
        Err(ERROR_INSUFFICIENT_BUFFER.0)
    }

    pub fn clear(&mut self) {
        self.cached = None;
    }

    pub fn sample(&mut self, utc_ms: u64) -> Arc<EndpointSnapshot> {
        if let Some((at, value)) = &self.cached
            && at.elapsed() < Duration::from_secs(5)
        {
            return value.clone();
        }
        let mut entries = Vec::new();
        let mut omitted = 0;
        // SAFETY: fetch supplies a writable buffer of the stated size or null for sizing.
        let tcp = |af| {
            move |buf, len: &mut u32| unsafe {
                GetExtendedTcpTable(buf, len, false, af, TCP_TABLE_OWNER_PID_ALL, 0)
            }
        };
        let udp = |af| {
            move |buf, len: &mut u32| unsafe {
                GetExtendedUdpTable(buf, len, false, af, UDP_TABLE_OWNER_PID, 0)
            }
        };
        let mut tcp_row = |local, remote, state, pid| {
            entries.push(Endpoint {
                transport: Transport::Tcp,
                local,
                remote: (state != MIB_TCP_STATE_LISTEN.0 as u32).then_some(remote),
                tcp_state: Some(state),
                pid,
            })
        };
        // SAFETY: each parser is paired with the exact IP Helper table class and family.
        let results = unsafe {
            let t4 = self.fetch(tcp(u32::from(AF_INET.0))).and_then(|bytes| {
                parse(
                    bytes,
                    offset_of!(MIB_TCPTABLE_OWNER_PID, table),
                    |r: MIB_TCPROW_OWNER_PID| {
                        tcp_row(
                            v4(r.dwLocalAddr, r.dwLocalPort),
                            v4(r.dwRemoteAddr, r.dwRemotePort),
                            r.dwState,
                            r.dwOwningPid,
                        );
                    },
                )
            });
            let t6 = self.fetch(tcp(u32::from(AF_INET6.0))).and_then(|bytes| {
                parse(
                    bytes,
                    offset_of!(MIB_TCP6TABLE_OWNER_PID, table),
                    |r: MIB_TCP6ROW_OWNER_PID| {
                        tcp_row(
                            v6(r.ucLocalAddr, r.dwLocalPort, r.dwLocalScopeId),
                            v6(r.ucRemoteAddr, r.dwRemotePort, r.dwRemoteScopeId),
                            r.dwState,
                            r.dwOwningPid,
                        );
                    },
                )
            });
            let mut udp_row = |local, pid| {
                entries.push(Endpoint {
                    transport: Transport::Udp,
                    local,
                    remote: None,
                    tcp_state: None,
                    pid,
                })
            };
            let u4 = self.fetch(udp(u32::from(AF_INET.0))).and_then(|bytes| {
                parse(
                    bytes,
                    offset_of!(MIB_UDPTABLE_OWNER_PID, table),
                    |r: MIB_UDPROW_OWNER_PID| {
                        udp_row(v4(r.dwLocalAddr, r.dwLocalPort), r.dwOwningPid)
                    },
                )
            });
            let u6 = self.fetch(udp(u32::from(AF_INET6.0))).and_then(|bytes| {
                parse(
                    bytes,
                    offset_of!(MIB_UDP6TABLE_OWNER_PID, table),
                    |r: MIB_UDP6ROW_OWNER_PID| {
                        udp_row(
                            v6(r.ucLocalAddr, r.dwLocalPort, r.dwLocalScopeId),
                            r.dwOwningPid,
                        )
                    },
                )
            });
            [t4, t6, u4, u6]
        };
        let tables = results.map(|r| match r {
            Ok((kept, dropped)) => {
                omitted += dropped;
                Availability::Available(kept)
            }
            Err(code) => status(code),
        });
        entries.sort_unstable_by_key(|r| (r.pid, r.transport, r.local, r.remote));
        let snapshot = Arc::new(EndpointSnapshot {
            utc_ms,
            entries,
            tables,
            omitted,
        });
        self.cached = Some((Instant::now(), snapshot.clone()));
        snapshot
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn finds_our_tcp_listener_and_udp_socket_without_sending_traffic() {
        let tcp = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let udp = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let pair = |a, p| (a, p);
        let mut expected = vec![
            pair(tcp.local_addr().unwrap(), Transport::Tcp),
            pair(udp.local_addr().unwrap(), Transport::Udp),
        ];
        let ipv6 = std::net::TcpListener::bind("[::1]:0").ok();
        if let Some(tcp) = &ipv6 {
            expected.push((tcp.local_addr().unwrap(), Transport::Tcp));
        }
        let result = Endpoints::default().sample(1);
        for (addr, proto) in expected {
            assert!(
                result.entries.iter().any(|r| r.pid == std::process::id()
                    && r.local == addr
                    && r.transport == proto
                    && r.remote.is_none()),
                "{addr} {proto:?} — {:?}",
                result.tables
            );
        }
    }
}
