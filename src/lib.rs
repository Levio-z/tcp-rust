use std::collections::{HashMap, VecDeque};
use std::io;
use std::io::prelude::*;
use std::net::Ipv4Addr;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
mod tcp;

const SENDQUEUE_SIZE: usize = 1024;

#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq)]
struct Quad {
    src: (Ipv4Addr, u16),
    dst: (Ipv4Addr, u16),
}

pub type Result<T> = core::result::Result<T, io::Error>;
type InterfaceHandle = Arc<Foobar>;

#[derive(Default)]
struct Foobar {
    manager: Mutex<ConnectionManager>,
    pending_var: Condvar,
}

#[derive(Default)]
struct ConnectionManager {
    terminate: bool,
    connections: HashMap<Quad, tcp::Connection>,
    // 记录 每个端口对应的等待连接队列
    pending: HashMap<u16, VecDeque<Quad>>,
}

pub struct Interface {
    ih: Option<InterfaceHandle>,
    jh: Option<thread::JoinHandle<io::Result<()>>>,
}

impl Interface {
    pub fn new() -> io::Result<Self> {
        let nic = tun_tap::Iface::without_packet_info("tun0", tun_tap::Mode::Tun)?;

        let ih: InterfaceHandle = Arc::default();
        //启动一个额外的线程
        let jh = {
            //需要保留一个连接管理器的副本
            let ih = ih.clone();
            thread::spawn(move || packet_loop(nic, ih))
        };

        Ok(Interface {
            ih: Some(ih),
            jh: Some(jh),
        })
    }
    pub fn bind(&mut self, port: u16) -> io::Result<TcpListener> {
        use std::collections::hash_map::Entry;
        let mut cm = self.ih.as_mut().unwrap().manager.lock().unwrap();
        //检测次端口之前是否绑定过
        match cm.pending.entry(port) {
            Entry::Vacant(v) => {
                v.insert(VecDeque::new());
            }
            Entry::Occupied(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    "port already bound",
                ));
            }
        };
        drop(cm);
        Ok(TcpListener {
            port,
            h: self.ih.as_mut().unwrap().clone(),
        })
    }
}
fn packet_loop(mut nic: tun_tap::Iface, ih: InterfaceHandle) -> io::Result<()> {
    let mut buf = [0u8; 1504];

    loop {
        // TODO: set a timeout for this recv for TCP timers or ConnectionManager::terminate
        let nbytes = nic.recv(&mut buf[..])?;

        // TODO: if self.terminate && Arc::get_strong_refs(ih) == 1; then tear down all connections and return.

        // if s/without_packet_info/new/:
        //
        // let _eth_flags = u16::from_be_bytes([buf[0], buf[1]]);
        // let eth_proto = u16::from_be_bytes([buf[2], buf[3]]);
        // if eth_proto != 0x0800 {
        //     // not ipv4
        //     continue;
        // }
        //
        // and also include on send

        match etherparse::Ipv4HeaderSlice::from_slice(&buf[..nbytes]) {
            Ok(iph) => {
                let src = iph.source_addr();
                let dst = iph.destination_addr();
                if iph.protocol() != etherparse::IpNumber::IPV6 {
                    eprintln!("BAD PROTOCOL");
                    // not tcp
                    continue;
                }

                match etherparse::TcpHeaderSlice::from_slice(&buf[iph.slice().len()..nbytes]) {
                    Ok(tcph) => {
                        use std::collections::hash_map::Entry;
                        let datai = iph.slice().len() + tcph.slice().len();
                        let mut cmg = ih.manager.lock().unwrap();
                        let mut cm= &mut *cmg;
                        let q = Quad {
                            src: (src, tcph.source_port()),
                            dst: (dst, tcph.destination_port()),
                        };

                        match cm.connections.entry(q) {
                            Entry::Occupied(mut c) => {
                                eprintln!("got packet for known quad {:?}", q);
                                c.get_mut()
                                    .on_packet(&mut nic, iph, tcph, &buf[datai..nbytes])?;
                            }
                            Entry::Vacant(e) => {
                                eprintln!("got packet for unknown quad {:?}", q);
                                // 接收到发送数据，是否有人在监听这个端口
                                // 有这个端口的监听器吗？
                                if let Some(pending) = cm.pending.get_mut(&tcph.destination_port())
                                {
                                    eprintln!("listening, so accepting");
                                    if let Some(c) = tcp::Connection::accept(
                                        &mut nic,
                                        iph,
                                        tcph,
                                        &buf[datai..nbytes],
                                    )? {
                                        e.insert(c);
                                        pending.push_back(q);
                                        drop(cmg);
                                        // 通知所有等待者：“有新任务啦！”
                                        ih.pending_var.notify_all()
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("ignoring weird tcp packet {:?}", e);
                    }
                }
            }
            Err(e) => {
                eprintln!("ignoring weird packet {:?}", e);
            }
        }
    }
}

pub struct TcpStream {
    quad: Quad,
    h: InterfaceHandle,
}

impl Read for TcpStream {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
       let mut cm = self.h.manager.lock().unwrap();
       // 从连接池获取连接
       let c = cm.connections.get_mut(&self.quad).ok_or_else(||{
        io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "streams was terminated unexpectedly",
            )
       })?;
       // 没有数据
       if c.incoming.is_empty(){
        //todo：clock 超时控制
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "no bytes to read",
        ));
       }
        // TODO:detect Fin and return nread==0
        let mut nread = 0;
        //从环形队列读取数据
        let (head, tail) = c.incoming.as_slices();
        let hread = std::cmp::min(buf.len(), head.len());
        buf.copy_from_slice(&head[..hread]);
        nread += hread;
        let tread = std::cmp::min(buf.len() - nread, tail.len());
        buf.copy_from_slice(&tail[..tread]);
        nread += tread;
        drop(c.incoming.drain(..nread));
        Ok(nread)
    }
}
impl Write for TcpStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut cm = self.h.manager.lock().unwrap();
        let c = cm.connections.get_mut(&self.quad).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "stream was terminated unexpectedly",
            )
        })?;

        if c.unacked.len() >= SENDQUEUE_SIZE {
            // TODO: block 直到队列中的一些字节在网络上发出去
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "too many bytes buffered",
            ));
        }

        let nwrite = std::cmp::min(buf.len(), SENDQUEUE_SIZE - c.unacked.len());
        c.unacked.extend(buf[..nwrite].iter());

        // TODO: wake up writer

        Ok(nwrite)
    }
    fn flush(&mut self) -> io::Result<()> {
        let mut cm = self.h.manager.lock().unwrap();
        let c = cm.connections.get_mut(&self.quad).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "stream was terminated unexpectedly",
            )
        })?;

        if c.unacked.is_empty() {
            Ok(())
        } else {
            // TODO: block
            Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "too many bytes buffered",
            ))
        }
    }
}

impl TcpStream {
    pub fn shutdown(&self, how: std::net::Shutdown) -> Result<()> {
        // TODO: send FIN on cm.connections[quad]
        unimplemented!()
    }
}

pub struct TcpListener {
    port: u16,
    h: InterfaceHandle,
}
impl TcpListener {
    pub fn accept(&mut self) -> Result<TcpStream> {
        let mut cm = self.h.manager.lock().unwrap();
        loop {
            if let Some(quad) = cm
                .pending
                .get_mut(&self.port)
                .expect("port closed while listener still active")
                .pop_front()
            {
                return Ok(TcpStream {
                    quad,
                    h: self.h.clone(),
                });
            }

            cm = self.h.pending_var.wait(cm).unwrap();
        }
    }
}


impl Drop for TcpStream {
    fn drop(&mut self) {
        let mut cm = self.h.manager.lock().unwrap();
        // TODO: send FIN on cm.connections[quad]
        // TODO: _eventually_ remove self.quad from cm.connections
        // like shutdown
    }
}

impl Drop for TcpListener {
    fn drop(&mut self) {
        let mut cm = self.h.manager.lock().unwrap();

        let pending = cm
            .pending
            .remove(&self.port)
            .expect("port closed while listener still active");

        for quad in pending {
            // TODO: terminate cm.connections[quad]
            unimplemented!();
        }
    }
}
impl Drop for Interface {
    fn drop(&mut self) {
        self.ih.as_mut().unwrap().manager.lock().unwrap().terminate = true;

        drop(self.ih.take());
        self.jh
            .take()
            .expect("interface dropped more than once")
            .join()
            .unwrap()
            .unwrap();
    }
}