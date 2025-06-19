use tun_tap::Mode;
mod tcp;
use core::net::Ipv4Addr;
use std::collections::hash_map::Entry;
use std::{collections::HashMap, io};

#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq)]
struct Quad {
    src: (Ipv4Addr, u16),
    dst: (Ipv4Addr, u16),
}
fn main() -> io::Result<()> {
    // 从名为 tun0 的虚拟网络接口中不断读取 IP 数据包
    // 创建并打开名为 tun0 的 TUN 虚拟网络接口
    // 成功后返回一个 Iface 实例，它实际上是一个对 /dev/net/tun 文件描述符的封装。
    let mut nic = tun_tap::Iface::without_packet_info("tun0", Mode::Tun)?;
    let mut buf = [0u8; 1504];
    let mut connections: HashMap<Quad, tcp::Connection> = HashMap::new();
    loop {
        // 接受数据
        let nbytes = nic.recv(&mut buf).unwrap();
        // 用于解析网络包中的各种协议头（如 Ethernet、IPv4、TCP、UDP 等）。
        match etherparse::Ipv4HeaderSlice::from_slice(&buf[..nbytes]) {
            Ok(iph) => {
                let src = iph.source_addr();

                let dst = iph.destination_addr();
                // 打印 IP 包的源 IP、目标 IP 以及协议类型（iph.protocol()，例如 TCP=6、UDP=17）。
                eprintln!(
                    "src:{:?}dst:{:?}iph.protocol:{:?}",
                    src.to_string(),
                    dst.to_string(),
                    iph.protocol()
                );
                // 如果不是 TCP 协议，则跳过当前包。
                if iph.protocol() != etherparse::IpNumber::TCP {
                    eprintln!("BAD PROTOCOL");

                    // not tcp

                    continue;
                }
                // 获取 IP 头部长度
                let ip_header_len = iph.slice().len();

                match etherparse::TcpHeaderSlice::from_slice(&buf[ip_header_len..nbytes]) {
                    Ok(tcph) => {
                        let src_port = tcph.source_port();
                        let dst_port = tcph.destination_port();
                        let seq = tcph.sequence_number();
                        let ack = tcph.acknowledgment_number();

                        let datai = iph.slice().len() + tcph.slice().len();
                        match connections.entry(Quad {
                            src: (src, tcph.source_port()),
                            dst: (dst, tcph.destination_port()),
                        }) {
                            Entry::Occupied(mut c) => {
                                eprintln!("===============接收到数据=============");
                                eprintln!(
                                    "TCP src_port: {}, dst_port: {}, seq: {}, ack: {}",
                                    src_port, dst_port, seq, ack
                                );
                                c.get_mut()
                                    .on_packet(&mut nic, iph, tcph, &buf[datai..nbytes])?;
                            }
                            Entry::Vacant(mut e) => {
                                if let Some(c) = tcp::Connection::accept(
                                    &mut nic,
                                    iph,
                                    tcph,
                                    &buf[datai..nbytes],
                                )? {
                                    e.insert(c);
                                }
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("Failed to parse TCP header: {:?}", e);
                    }
                }
            }

            Err(e) => {
                eprintln!("ignoring weird packet {:?}", e);
            }
        }
    }
}
