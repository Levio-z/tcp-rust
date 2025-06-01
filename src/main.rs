use tun_tap::{Iface, Mode};
fn main() {
    let iface = tun_tap::Iface::new("tun0", Mode::Tun).unwrap();
    println!("Hello, world!");
    loop {
    let mut buf = [0u8; 1500];
    let len = iface.recv(&mut buf).unwrap();
    println!("Received {} bytes", len);
    println!("{:?}", buf);
    }
}
