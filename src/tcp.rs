use std::io;
use std::io::Write;
use std::ops::Sub;

pub enum State {
    //Closed,
    //Listen,
    SynRcvd,
    Estab,
    FinWait1,
    FinWait2,
    TimeWait,
}
pub struct Connection {
    state: State,
    send: SendSequenceSpace,
    recv: RecvSequenceSpace,
    ip: etherparse::Ipv4Header,
    tcp: etherparse::TcpHeader,
}
/// State of the Send Sequence Space (RFC 793 S3.2 F4)
///
/// ```
///            1         2          3          4
///       ----------|----------|----------|----------
///              SND.UNA    SND.NXT    SND.UNA
///                                   +SND.WND
///
/// 1 - old sequence numbers which have been acknowledged
/// 2 - sequence numbers of unacknowledged data
/// 3 - sequence numbers allowed for new data transmission
/// 4 - future sequence numbers which are not yet allowed
/// ```
struct SendSequenceSpace {
    /// send unacknowledged
    una: u32,
    /// send next
    nxt: u32,
    /// send window
    wnd: u16,
    /// send urgent pointer
    up: bool,
    /// segment sequence number used for last window update
    wl1: usize,
    /// segment acknowledgment number used for last window update
    wl2: usize,
    /// initial send sequence number
    iss: u32,
}

/// State of the Receive Sequence Space (RFC 793 S3.2 F5)
///
/// ```Add commentMore actions
///                1          2          3
///            ----------|----------|----------
///                   RCV.NXT    RCV.NXT
///                             +RCV.WND
///
/// 1 - old sequence numbers which have been acknowledged
/// 2 - sequence numbers allowed for new reception
/// 3 - future sequence numbers which are not yet allowed
/// ```
struct RecvSequenceSpace {
    /// receive next
    nxt: u32,
    /// receive window
    wnd: u16,
    /// receive urgent pointer
    up: bool,
    /// initial receive sequence number
    irs: u32,
}

impl Connection {
    // receive new connection
    pub fn accept<'a>(
        nic: &mut tun_tap::Iface,
        iph: etherparse::Ipv4HeaderSlice<'a>,
        tcph: etherparse::TcpHeaderSlice<'a>,
        data: &'a [u8],
    ) -> io::Result<Option<Self>> {
        let mut buf = [0u8; 1500];

        if !tcph.syn() {
            // only excepted SYN packet
            return Ok(None);
        }
        let iss: u32 = 0;
        let wnd = 1024;
        let mut c = Connection {
            state: State::SynRcvd,
            // decide on stuff we're sending them
            send: SendSequenceSpace {
                iss,
                una: iss,
                nxt: iss,
                wnd: wnd,

                up: false,
                wl1: 0,
                wl2: 0,
            },
            // keep track of sender info
            recv: RecvSequenceSpace {
                irs: tcph.sequence_number(),
                nxt: tcph.sequence_number() + 1,
                wnd: tcph.window_size(),
                up: false,
            },
            tcp: etherparse::TcpHeader::new(tcph.destination_port(), tcph.source_port(), iss, wnd),
            ip: etherparse::Ipv4Header::new(
                0,
                64,
                etherparse::IpNumber::TCP,
                iph.destination(),
                iph.source(),
            )
            .unwrap(),
        };
        // need to start establishing a connectionAdd commentMore actions
        c.tcp.syn = true;
        c.tcp.ack = true;
        eprintln!("connection is estabed");
        c.write(nic, &[])?;
        Ok(Some(c))
    }
    fn write(&mut self, nic: &mut tun_tap::Iface, payload: &[u8]) -> io::Result<usize> {
        let mut buf = [0u8; 1500];
        // 设置 TCP 报文的当前 SEQ 和 ACK 字段
        // 是当前待发送的 sequence number
        self.tcp.sequence_number = self.send.nxt;
        // 是我们期望接收的下一个字节（即 ack number）
        self.tcp.acknowledgment_number = self.recv.nxt;

        // 设置载荷
        let size = std::cmp::min(
            buf.len(),
            self.tcp.header_len() as usize + self.ip.header_len() as usize + payload.len(),
        );
        self.ip
            .set_payload_len(size - self.ip.header_len() as usize);

        // 计算校验和
        self.tcp.checksum = self
            .tcp
            .calc_checksum_ipv4(&self.ip, &[])
            .expect("failed to compute checksum");

        // 写入 IP + TCP 头 + Payload 到缓冲区
        let mut unwritten = &mut buf[..];
        self.ip.write(&mut unwritten);
        self.tcp.write(&mut unwritten);
        let payload_bytes = unwritten.write(payload)?;

        // 根据有效载荷更新发送序列号 SND.NXT
        let unwritten = unwritten.len(); // 记录剩余 buffer 大小
        // 新发送序列号 = 旧发送序列号+载荷
        self.send.nxt = self.send.nxt.wrapping_add(payload_bytes as u32);
        // syn和fin比较特殊也算有效载荷,这两个不是每次都需要的公有字段，使用完需要清空
        if self.tcp.syn {
            self.send.nxt = self.send.nxt.wrapping_add(1);
            self.tcp.syn = false;
        }
        if self.tcp.fin {
            self.send.nxt = self.send.nxt.wrapping_add(1);
            self.tcp.fin = false;
        }
        println!("{:0x?}", &buf[..buf.len() - unwritten]);
        nic.send(&buf[..buf.len() - unwritten])
    }
    pub fn on_packet<'a>(
        &mut self,
        nic: &mut tun_tap::Iface,
        iph: etherparse::Ipv4HeaderSlice<'a>,
        tcph: etherparse::TcpHeaderSlice<'a>,
        data: &'a [u8],
    ) -> io::Result<()> {
        // SEG.SEQ
        let seqn = tcph.sequence_number();
        // SEG.LEN
        let mut slen = data.len() as u32;

        // SYN 和 FIN 虽然**不携带数据**，但它们必须占用一个序列号 —— 不是因为它们有数据，而是因为它们**具有语义上的“有效载荷”作用**,
        // 有效载荷就需要占用序列号，会让接收方窗口移动，只有有效载荷必须要使用ack确认
        if tcph.fin() {
            slen += 1;
        };
        if tcph.syn() {
            slen += 1;
        };

        println!("recv.nxt:{}self.recv.wnd:{}seqn:{}slen:{}",self.recv.nxt, self.recv.wnd, seqn, slen);
        if !segment_valid(self.recv.nxt, self.recv.wnd, seqn, slen) {
            eprintln!("!segment_valid");
            self.write(nic, &[])?;
            return Ok(());
        }

        // 接受方窗口移动
        self.recv.nxt = seqn.wrapping_add(slen);

        if !tcph.ack() {
            return Ok(());
        }
        let ackn = tcph.acknowledgment_number();
        if let State::SynRcvd = self.state {
            // 冗余确认是快速重传机制的关键触发器
            // 握手阶段设计更宽松以保证连接建立可靠
            if is_between_wrapped(self.send.una.sub(1), ackn, self.send.nxt.wrapping_add(1)) {
                // 我们判断客户端发送的 ACK 报文，确认了我们发出的 SYN 报文。
                // 我们并未发送任何数据，唯一发送的就是 SYN 报文，其本身占用一个序号。因此，只要对方确认的 ack number 超过初始序列号，就意味着 SYN 被确认。
                eprintln!("连接建立");
                self.state = State::Estab;
            } else {
                // TODO: <SEQ=SEG.ACK><CTL=RST>    and send it.  并发送它。

            }
        }

        if let State::Estab | State::FinWait1= self.state {
            if !is_between_wrapped(self.send.una, ackn, self.send.nxt.wrapping_add(1)) {
                println!("{}-{}-{}",self.send.una, ackn, self.send.nxt.wrapping_add(1));
                println!("!is_between_wrapped");
                // TODO 如果 ACK 确认尚未发送的内容 （SEG.ACK > SND.NXT），然后发送一个 ACK，删除该段，然后返回。
                return Ok(());
            }
            //  发送数据，开始收到回复，需要更新发送窗口边界
            self.send.una = ackn;
            // TODO 如果 SND.UNA < SEG.ACK =< SND。NXT，发送窗口应该更新。发送窗口应该更新
            // TODO
            assert!(data.is_empty());

            if let State::Estab = self.state {
                // now let's terminate the connection!
                // TODO: needs to be stored in the retransmission queue!
                self.tcp.fin = true;
                self.write(nic, &[])?;
                eprintln!("Estab->FinWait1");
                self.state = State::FinWait1;
            }
        }

        if let State::FinWait1 = self.state {
            // // 只能收到两次ack
            // if self.send.una == self.send.iss + 2 {
                // our FIN has been ACKed!
                eprintln!("FinWait1->FinWait2");
                self.state = State::FinWait2;
            // }
        }
        // FinWait2不会判断ack了，FinWait1->FinWait2，我发出的最后一个命令已经被ack了，对面不会修改ack
        if tcph.fin() {
            match self.state {
                State::FinWait2 => {           
                    // 确定用户的close 但不会删除TCB     
                    // we're done with the connection!
                    self.write(nic, &[])?;
                    self.state = State::TimeWait;
                    eprintln!("we're done with the connection!");
                }
                _ => unimplemented!(),
            }
        }

        return Ok(());
    }
}
fn is_between_wrapped(start: u32, x: u32, end: u32) -> bool {
    x != start && (end.wrapping_sub(start) > x.wrapping_sub(start))
}

/// ```
/// | Segment Length (`SEG.LEN`) | Window Size (`RCV.WND`) | 接收判断条件                                                                                                   
/// | -------------------------- | ----------------------- | --------------------------------------------------------------------------------------------------------     |
/// | 0                          | 0                       | `SEG.SEQ == RCV.NXT`                                                                                         |
/// | 0                          | >0                      | `RCV.NXT <= SEG.SEQ < RCV.NXT + RCV.WND`                                                                     |
/// | >0                         | 0                       | 不合法（被丢弃）                                                                                               |
/// | >0                         | >0                      | `RCV.NXT =< SEG.SEQ < RCV.NXT+RCV.WND  or `RCV.NXT =< SEG.SEQ+SEG.LEN-1 < RCV.NXT+RCV.WND`                   |
/// ```
fn segment_valid(recv_nxt: u32, recv_wnd: u16, seqn: u32, slen: u32) -> bool {
    let wend = recv_nxt.wrapping_add(recv_wnd as u32);
    if slen == 0 {
        if recv_wnd == 0 {
            seqn == recv_nxt
        } else {
            seq_in_window(recv_nxt, wend, seqn)
        }
    } else {
        if recv_wnd == 0 {
            false
        } else {
            seq_in_window(recv_nxt, wend, seqn)
                || seq_in_window(recv_nxt, wend, seqn.wrapping_add(slen - 1))
        }
    }
}
fn seq_in_window(start: u32, end: u32, val: u32) -> bool {
    is_between_wrapped(start.wrapping_sub(1), val, end)
}
