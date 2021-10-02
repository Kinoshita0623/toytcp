use anyhow::Result;
use std::{env, io, net::Ipv4Addr, str};
use toytcp::tcp::TCP;

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    let addr: Ipv4Addr = args[1].parse()?;
    let port: u16 = args[2].parse()?;
    echo_server(addr, port)?;
    Ok(())
}

fn echo_server(local_addr: Ipv4Addr, local_port: u16) -> Result<()> {
    let tcp = TCP::new();
    let listening_socket = tcp.listen(local_addr, local_port)?;
    dbg!("listeing..");
    loop {
        let connected_socket = tcp.accept(listening_socket)?;
        dbg!("accepted!", connected_socket.1, connected_socket.3);
    }
}



// 221 103916.822045533     10.0.1.1 → 10.0.0.1     TCP 74 37930 → 40000 [SYN] Seq=0 Win=64240 Len=0 MSS=1460 SACK_PERM=1 TSval=179712802 TSecr=0 WS=128
// 222 103916.824420473     10.0.0.1 → 10.0.1.1     TCP 54 40000 → 37930 [SYN, ACK] Seq=0 Ack=1 Win=4380 Len=0
// 223 103916.824485220     10.0.1.1 → 10.0.0.1     TCP 54 37930 → 40000 [ACK] Seq=1 Ack=1 Win=64240 Len=0
// 224 103918.978018398     10.0.1.1 → 10.0.0.1     TCP 54 [TCP Retransmission] 37928 → 40000 [FIN, ACK] Seq=1 Ack=1 Win=64240 Len=0
// 225 103929.735007711     10.0.1.1 → 10.0.0.1     TCP 54 [TCP Retransmission] 37906 → 40000 [FIN, ACK] Seq=1 Ack=1 Win=64240 Len=0
// 226 103946.118917828     10.0.1.1 → 10.0.0.1     TCP 54 [TCP Retransmission] 37928 → 40000 [FIN, ACK] Seq=1 Ack=1 Win=64240 Len=0