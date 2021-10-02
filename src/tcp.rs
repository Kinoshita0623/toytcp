use crate::packet::TCPPacket;
use crate::socket::{SocketID, Socket, TcpStatus};
use crate::tcpflags;
use anyhow::{Context, Result};
use pnet::packet::{ip::IpNextHeaderProtocols, tcp::TcpPacket, Packet};
use pnet::transport::{self, TransportChannelType};
use rand::{rngs::ThreadRng, Rng};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::process::Command;
use std::sync::{Arc, Condvar, Mutex, RwLock, RwLockWriteGuard};
use std::time::{Duration, SystemTime};
use std::{cmp, ops::Range, str, thread};

const UNDETERMINED_IP_ADDR: std::net::Ipv4Addr = Ipv4Addr::new(0, 0, 0, 0);
const UNDETERMINED_PORT: u16 = 0;
const MAX_TRANSMITTION: u8 = 5;
const RETRANSMITTION_TIMEOUT: u64 = 3;
const MSS: usize = 1460;
const PORT_RANGE: Range<u16> = 40000..60000;

pub struct TCP {
    sockets: RwLock<HashMap<SocketID, Socket>>,
    event_condvar: (Mutex<Option<TCPEvent>>, Condvar)
}

impl TCP {

    pub fn new() -> Arc<Self> {
        let sockets = RwLock::new(HashMap::new());
        let tcp = Arc::new(Self{
            sockets,
            event_condvar: (Mutex::new(None), Condvar::new())
        });
        let cloned_tcp = tcp.clone();
        std::thread::spawn(move || {
            cloned_tcp.receive_handler().unwrap();
        });

        let cloned_tcp = tcp.clone();
        std::thread::spawn(move || {
            cloned_tcp.timer();
        });
        return tcp;
    }


    /**
     * アクティブオープンの場合に使用されるメソッド
     */
    pub fn connect(&self, addr: Ipv4Addr, port: u16) -> Result<SocketID> {
        let mut rng = rand::thread_rng();
        let mut socket = Socket::new(
            get_source_addr_to(addr)?,
            addr,
            self.select_unused_port(&mut rng)?,
            port,
            TcpStatus::SynSent
        )?;

        socket.send_param.initial_seq = rng.gen_range(1..1 << 31);
        dbg!("initial_seq:", socket.send_param.initial_seq);
        socket.send_tcp_packet(socket.send_param.initial_seq, 0, tcpflags::SYN, &[])?;
        socket.send_param.unacked_seq = socket.send_param.initial_seq;
        socket.send_param.next = socket.send_param.initial_seq + 1;
        let mut table =self.sockets.write().unwrap();
        let socket_id = socket.get_socket_id();

        table.insert(socket_id, socket);
        // ロックを解除してイベントの待機。
        // 受信スレッドがロックを獲得できるようにするため。
        drop(table);
        self.wait_event(socket_id, TCPEventKind::ConnectionCompleted);
        return Ok(socket_id);
    }

    /**
     * Listen状態のSocketを生み出す。
     */
    pub fn listen(&self, local_addr: Ipv4Addr, local_port: u16) -> Result<SocketID> {
        let socket = Socket::new(
            local_addr,
            UNDETERMINED_IP_ADDR,
            local_port,
            UNDETERMINED_PORT,
            TcpStatus::Listen
        )?;
        let mut lock = self.sockets.write().unwrap();
        let socket_id = socket.get_socket_id();
        lock.insert(socket_id, socket);
        return Ok(socket_id);
    }

    /**
     * 接続済みSocketが生成されるまで待ち受ける
     */
    pub fn accept(&self, socket_id: SocketID) -> Result<SocketID> {
        self.wait_event(socket_id, TCPEventKind::ConnectionCompleted);
        let mut table = self.sockets.write().unwrap();
        return Ok(
            table
            .get_mut(&socket_id)
            .context(format!("no such socket: {:?}", socket_id))?
            .connected_connection_queue
            .pop_front()
            .context("no connected socket")?
        );
    }

    pub fn send(&self, socket_id: SocketID, buffer: &[u8]) -> Result<()> {
        let mut cursor = 0;
        while cursor < buffer.len() {
            let mut table = self.sockets.write().unwrap();
            let mut socket = table.get_mut(&socket_id)
                .context(format!("no such socket: {:?}", socket_id))?;
            let send_size = cmp::min(MSS, buffer.len() - cursor);
            socket.send_tcp_packet(
                socket.send_param.next,
                socket.recv_param.next,
                tcpflags::ACK,
                &buffer[cursor..cursor + send_size],
            )?;
            cursor += send_size;
            socket.send_param.next += send_size as u32;
        }
        return Ok(());
    }
    
    fn select_unused_port(&self, rng: &mut ThreadRng) -> Result<u16> {
        for _ in 0..(PORT_RANGE.end - PORT_RANGE.start) {
            let local_port = rng.gen_range(PORT_RANGE);
            let table = self.sockets.read().unwrap();
            if table.keys().all(|k| local_port != k.2) {
                return Ok(local_port);
            }
        }
        anyhow::bail!("no avaiable port found.");
    }

    fn receive_handler(&self) -> Result<()> {
        dbg!("begin recv thread");
        let (_, mut receiver) = transport::transport_channel(
            65535,
            TransportChannelType::Layer3(IpNextHeaderProtocols::Tcp),
        )?;
        let mut packet_iter = transport::ipv4_packet_iter(&mut receiver);
        loop {
            // 次のパケットが来るまでスレッドをブロックして待つ
            let (packet, remote_addr) = match packet_iter.next() {
                Ok((p, r)) => (p,r ), 
                Err(_) => continue,
            };
            let local_addr = packet.get_destination();
            let tcp_packet = match TcpPacket::new(packet.payload()) {
                Some(p) => p,
                None => {
                    continue;
                }
            };
            let packet = TCPPacket::from(tcp_packet);
            let remote_addr = match remote_addr {
                IpAddr::V4(addr) => addr,
                _ => {
                    continue;
                }
            };
            let mut table = self.sockets.write().unwrap();

            // パケットが管轄内であるか？
            let socket = match table.get_mut(
                &SocketID(
                    local_addr,
                    remote_addr,
                    packet.get_dest(),
                    packet.get_src()
                )
            ) {
                Some(socket) => socket, // 接続済みソケット
                None => match table.get_mut(
                    &SocketID(
                        local_addr,
                        UNDETERMINED_IP_ADDR,
                        packet.get_dest(),
                        UNDETERMINED_PORT
                    ) 
                ) {
                    Some(socket) => socket, // リスニングソケット
                    None => continue,
                }
            };

            if !packet.is_correct_checksum(local_addr, remote_addr) {
                dbg!("invalid checksum");
                continue;
            }
            let socket_id = socket.get_socket_id();
            dbg!("received packet:", &packet);
            if let Err(error) = match socket.status {
                TcpStatus::Listen => self.listen_handler(table, socket_id, &packet, remote_addr),
                TcpStatus::SynRcvd => self.synrcvd_handler(table, socket_id, &packet),
                TcpStatus::SynSent => self.synsent_handler(socket, &packet),
                TcpStatus::Estableished => self.established_handler(socket, &packet),
                _ => {
                    dbg!("not implemented state:", &socket.status);
                    Ok(())
                }
            } {
                dbg!(error);
            }


        }

    }
    
    /**
     * syncを送信した後に受信されると呼び出される。
     */
    fn synsent_handler(&self, socket: &mut Socket, packet: &TCPPacket) -> Result<()> {
        dbg!("synsent handler");
        // SYN・ACKであること
        // ACK送信した時以上に大きいことをチェックしている
        //（送信したSYNパケットの次のパケットであることをチェックしている）
        if packet.get_flag() & tcpflags::ACK > 0 
            && socket.send_param.unacked_seq <= packet.get_ack() 
            && packet.get_ack() <= socket.send_param.next 
            && packet.get_flag() & tcpflags::SYN > 0 {

            socket.recv_param.next = packet.get_seq() + 1;
            socket.recv_param.initial_seq = packet.get_seq();
            socket.send_param.unacked_seq = packet.get_ack();
            socket.send_param.window = packet.get_window_size();
            if socket.send_param.unacked_seq > socket.send_param.initial_seq {
                socket.status = TcpStatus::Estableished;
                socket.send_tcp_packet(socket.send_param.next, socket.recv_param.next, tcpflags::ACK, &[])?;
                dbg!("status: synsent ->", &socket.status);
                self.publish_event(socket.get_socket_id(), TCPEventKind::ConnectionCompleted);
            } else {
                socket.status = TcpStatus::SynRcvd;
                socket.send_tcp_packet(socket.send_param.next, socket.recv_param.next, tcpflags::ACK, &[])?;
                dbg!("status: synsent ->", &socket.status);
            }
            
        }
        return Ok(());
    }

    /**
     * 現在の状態がLISTENの場合呼び出されるハンドラー。
     * 受診されたパケットのフラグがSYNだった場合はSYN・ACKを送信して状態をSynRcvdにする。
     */
    fn listen_handler(
        &self, mut table: RwLockWriteGuard<HashMap<SocketID, Socket>>, 
        listening_socket_id: SocketID, 
        packet: &TCPPacket, 
        remote_addr: Ipv4Addr
    ) -> Result<()> {
        dbg!("listen handler");
        if packet.get_flag() & tcpflags::ACK > 0 {

            // 本来ならRSTをsendする
            return Ok(());
        }
        
        let listening_socket = table.get_mut(&listening_socket_id).unwrap();

        // SYNを受信した場合
        if packet.get_flag() & tcpflags::SYN > 0 {
            // passive openの処理
            // 後に接続済みとなるソケットとなるソケットを新たに生成する
            let mut connection_socket = Socket::new(
                listening_socket.local_addr,
                remote_addr,
                listening_socket.local_port,
                packet.get_src(),
                TcpStatus::SynRcvd,
            )?;
            connection_socket.recv_param.next = packet.get_seq() + 1;
            connection_socket.recv_param.initial_seq = packet.get_seq();
            connection_socket.send_param.initial_seq = rand::thread_rng().gen_range(1..1 << 31);
            connection_socket.send_param.window = packet.get_window_size();

            // SYN・ACKを送信する
            connection_socket.send_tcp_packet(
                connection_socket.send_param.initial_seq,
                connection_socket.recv_param.next,
                tcpflags::SYN | tcpflags::ACK,
                &[]
            )?;
            connection_socket.send_param.next = connection_socket.send_param.initial_seq + 1;
            connection_socket.send_param.unacked_seq = connection_socket.send_param.initial_seq;
            connection_socket.listening_socket = Some(listening_socket.get_socket_id());
            dbg!("status: listen -> ", &connection_socket.status);
            table.insert(connection_socket.get_socket_id(), connection_socket);
        }
        return Ok(());

    }

    /**
     * 現在の状態がsynrcvdなときに呼び出されるハンドラー。
     * 受信したパケットがACKの場合はESTABLISHEDへ遷移する。
     */
    fn synrcvd_handler(
        &self,
        mut table: RwLockWriteGuard<HashMap<SocketID, Socket>>, 
        socket_id: SocketID,
        packet: &TCPPacket,
    ) -> Result<()> {

        dbg!("synrcvd handler");
        let socket = table.get_mut(&socket_id).unwrap();
        if packet.get_flag() & tcpflags::ACK > 0
            && socket.send_param.unacked_seq <= packet.get_ack()
            && packet.get_ack() <= socket.send_param.next {
            socket.recv_param.next = packet.get_seq();
            socket.send_param.unacked_seq = packet.get_ack();
            socket.status = TcpStatus::Estableished;
            dbg!("status: synrcvd ->", &socket.status);
            if let Some(id) = socket.listening_socket {
                let ls = table.get_mut(&id).unwrap();
                ls.connected_connection_queue.push_back(socket_id);
                self.publish_event(ls.get_socket_id(), TCPEventKind::ConnectionCompleted);
            }
        }
        return Ok(());
    }

    /**
     * ACKされて再送の必要性は無くなったためQueueから削除する。
     */
    fn delete_acked_segment_from_retransmission_queue(&self, socket: &mut Socket) {
        dbg!("ack accept", socket.send_param.unacked_seq);
        while let Some(item) = socket.retransmission_queue.pop_front() {
            if socket.send_param.unacked_seq > item.packet.get_seq() {
                dbg!("successfully acked", item.packet.get_seq());
                self.publish_event(socket.get_socket_id(), TCPEventKind::Acked);
            } else {
                // ackされていないので戻す。
                socket.retransmission_queue.push_front(item);
                break;
            }
        }
    }

    fn established_handler(&self, socket: &mut Socket, packet: &TCPPacket) -> Result<()> {
        dbg!("established handler");
        if socket.send_param.unacked_seq < packet.get_ack() 
            && packet.get_ack() <= socket.send_param.next {
            socket.send_param.unacked_seq = packet.get_ack();
            self.delete_acked_segment_from_retransmission_queue(socket);
        } else if socket.send_param.next < packet.get_ack() {
            // 未送信セグメントに対するACKは破棄
            return Ok(());
        }

        if packet.get_flag() & tcpflags::ACK == 0 {
            // ACKフラグが立っていないパッケトは破棄
            return Ok(());
        }
        Ok(())
    }

    fn wait_event(&self, socket_id: SocketID, kind: TCPEventKind) {
        let (lock, cvar) = &self.event_condvar;
        let mut event = lock.lock().unwrap();
        loop {
            if let  Some(ref e) = *event {
                if e.socket_id == socket_id && e.kind == kind {
                    break;
                }
            }
            event = cvar.wait(event).unwrap();
        }
        dbg!(&event);
        *event = None;
    }

    fn publish_event(&self, socket_id: SocketID, kind: TCPEventKind) {
        let (lock, cvar) = &self.event_condvar;
        let mut e = lock.lock().unwrap();
        *e = Some(TCPEvent::new(socket_id, kind));
        cvar.notify_all();
    }
    

    fn timer(&self) {
        dbg!("begin timer thread");
        loop {
            let mut table = self.sockets.write().unwrap();
            for (_, socket) in table.iter_mut() {
                while let Some(mut item) = socket.retransmission_queue.pop_front() {
                    if socket.send_param.unacked_seq > item.packet.get_seq() {
                        dbg!("successfully acked", item.packet.get_seq());
                        continue;
                    }

                    if item.latest_transmission_time.elapsed().unwrap() < Duration::from_secs(RETRANSMITTION_TIMEOUT) {
                        socket.retransmission_queue.push_front(item);
                        break;
                    }
                    if item.transmission_count < MAX_TRANSMITTION {
                        dbg!("retransmit");
                        socket.sender.send_to(item.packet.clone(), IpAddr::V4(socket.remote_addr))
                            .context("failed to retransmit")
                            .unwrap();
                        item.transmission_count += 1;
                        socket.retransmission_queue.push_back(item);
                        break;
                    } else {
                        // 再送回数の上限を超えたのでパケットは破棄する
                        dbg!("reached MAX_TRANSMISSION");
                    }

                }
            }
            drop(table);
            thread::sleep(Duration::from_millis(100));
        }
    }
}


fn get_source_addr_to(addr: Ipv4Addr) -> Result<Ipv4Addr> {
    //return Ok("10.0.0.1".parse().unwrap());
    let output = Command::new("sh")
        .arg("-c")
        .arg(format!("ip route get {} | grep src", addr))
        .output()?;
    let mut output = str::from_utf8(&output.stdout)?
        .trim()
        .split_ascii_whitespace();
        while let Some(s) = output.next() {
            if s == "src" {
                break;
            }
        }
        let ip = output.next().context("failed to get src ip")?;
        dbg!("source addr", ip);
        return ip.parse().context("failed to parse source ip");
}

#[derive(Debug, Clone, PartialEq)]
struct TCPEvent {
    socket_id: SocketID,
    kind: TCPEventKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TCPEventKind {
    ConnectionCompleted,
    Acked,
    DataArrived,
    ConnectionClosed,
}

impl TCPEvent {
    fn new(socket_id: SocketID, kind: TCPEventKind) -> Self {
        return Self {
            socket_id, kind,
        };
    }
}

