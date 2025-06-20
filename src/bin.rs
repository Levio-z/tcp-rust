use tun_tap::Mode;
mod tcp;
use core::net::Ipv4Addr;
use std::collections::hash_map::Entry;
use std::{collections::HashMap, io,thread};

#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq)]
struct Quad {
    src: (Ipv4Addr, u16),
    dst: (Ipv4Addr, u16),
}
fn main() -> io::Result<()> {
    let mut i = tcp_rust::Interface::new()?;
    eprintln!("created interface");
    let mut l1 = i.bind(7000)?;
    let jh = thread::spawn(move || {
        while let Ok(_stream) = l1.accept() {
            eprintln!("got connection!");
        }
    });
    jh.join().unwrap();
    Ok(())
}
