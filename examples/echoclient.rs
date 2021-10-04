use anyhow::Result;
use std::{env, io, net::Ipv4Addr, str};
use toytcp::tcp::TCP;

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    let addr: Ipv4Addr = args[1].parse()?;
    let port: u16 = args[2].parse()?;
    echo_client(addr, port)?;
    Ok(())
}


fn echo_client(remote_addr: Ipv4Addr, remote_port: u16) -> Result<()> {
    let tcp = TCP::new();
    let socket_id = tcp.connect(remote_addr, remote_port)?;
    loop {
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        loop {
            tcp.send(socket_id, input.repeat(2000).as_bytes())?;
        }
    }
}

// 09:06:14.839398 IP 10.0.0.1.42502 > 10.0.1.1.40000: Flags [S], seq 1781165593, win 4380, length 0
// 09:06:14.839458 IP 10.0.1.1.40000 > 10.0.0.1.42502: Flags [S.], seq 1358661489, ack 1781165594, win 64240, options [mss 1460], length 0
// 09:06:14.839656 IP 10.0.0.1.42502 > 10.0.1.1.40000: Flags [.], ack 1, win 4380, length 0