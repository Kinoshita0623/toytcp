use crate::packet::TCPPacket;
use crate::tcpflags;
use anyhow::{Context, Result};
use pnet::packet::{ip::IpNextHeaderProtocols, Packet};
use pnet::transport::{self, TransportChannelType, TransportProtocol, TransportSender};
use pnet::util;
use std::collections::VecDeque;
use std::fmt::{self, Display};
use std::net::{IpAddr, Ipv4Addr};
use std::time::SystemTime;

const SOCKET_BUFFER_SIZE: usize = 4380;

#[derive(Debug, Hash, Eq, PartialEq, Clone, Copy)]
pub struct SocketID(pub Ipv4Addr, pub Ipv4Addr, pub u16, pub u16);

/**
 * どこまで送信したかの情報を持つ
 */
#[derive(Clone, Debug)]
pub struct SendParam {
    pub unacked_seq: u32,   // 送信後まだACKされていないSequenceの先頭
    pub next: u32,  // 次の送信
    pub window: u16,    // 送信ウィンドウサイズ
    pub initial_seq: u32,
}

/**
 * どこまで受信したかの情報を持つ
 */
#[derive(Clone, Debug)]
pub struct RecvParam {
    pub next: u32,  // 次に受信するSequence
    pub window: u16,    // 受信ウィンドウ
    pub initial_seq: u32,   // 初期受信Sequence
    pub tail: u32,  // 受信Sequenceの最終尾
}

pub struct Socket {
    pub local_addr: Ipv4Addr,
    pub remote_addr: Ipv4Addr,
    pub local_port: u16,
    pub remote_port: u16,
    pub send_param: SendParam,
    pub recv_param: RecvParam,
    pub status: TcpStatus,
    pub connected_connection_queue: VecDeque<SocketID>,
    pub listening_socket: Option<SocketID>,
    pub sender: TransportSender,
}


impl Socket {
    pub fn new(
        local_addr: Ipv4Addr,
        remote_addr: Ipv4Addr,
        local_port: u16,
        remote_port: u16,
        status: TcpStatus,
    ) -> Result<Self> {
        let (sender, _) = transport::transport_channel(65535, TransportChannelType::Layer4(TransportProtocol::Ipv4(IpNextHeaderProtocols::Tcp)),)?;
        return Ok(Self {
            local_addr,
            remote_addr,
            local_port,
            remote_port,
            status: status,
            send_param: SendParam {
                unacked_seq: 0,
                initial_seq: 0,
                next: 0,
                window: SOCKET_BUFFER_SIZE as u16,
            },
            recv_param: RecvParam {
                initial_seq: 0,
                next: 0,
                window: SOCKET_BUFFER_SIZE as u16,
                tail: 0,
            },
            connected_connection_queue: VecDeque::new(),
            listening_socket: None,
            sender
        });
    }

    pub fn send_tcp_packet(&mut self, seq: u32, ack: u32, flag: u8, payload: &[u8]) -> Result<usize> {
        let mut tcp_packet = TCPPacket::new(payload.len());
        tcp_packet.set_src(self.local_port);
        tcp_packet.set_dest(self.remote_port);
        tcp_packet.set_seq(seq);
        tcp_packet.set_ack(ack);

        // オプションフィールドは使わないので固定（だそうです）
        tcp_packet.set_data_offset(5);
        tcp_packet.set_flag(flag);
        tcp_packet.set_window_size(self.recv_param.window);
        tcp_packet.set_checksum(util::ipv4_checksum(
            &tcp_packet.packet(),
            8,
            &[],
            &self.local_addr,
            &self.remote_addr,
            IpNextHeaderProtocols::Tcp,
        ));
        let sent_size = self.sender.send_to(tcp_packet.clone(), IpAddr::V4(self.remote_addr))
            .context(format!("failed to send: \n{:?}", tcp_packet))?;
        dbg!("sent", &tcp_packet);
        return Ok(sent_size);
    }

    pub fn get_socket_id(&self) -> SocketID {
        return SocketID(
            self.local_addr,
            self.remote_addr,
            self.local_port,
            self.remote_port
        );
    }
}

#[derive(PartialEq, Eq, Debug, Clone)]
pub enum TcpStatus {
    Listen,
    SynSent,
    SynRcvd,
    Estableished,
    FinWait1,
    FinWait2,
    TimeWait,
    CloseWait,
    LastAck,
}

impl Display for TcpStatus {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        return match self {
            TcpStatus::Listen => write!(f, "LISTEN"),
            TcpStatus::SynSent => write!(f, "SYNSENT"),
            TcpStatus::SynRcvd => write!(f, "SYNRCVD"),
            TcpStatus::Estableished => write!(f, "ESTABLISHED"),
            TcpStatus::FinWait1 => write!(f, "FINWAIT1"),
            TcpStatus::FinWait2 => write!(f, "FINWAIT2"),
            TcpStatus::TimeWait => write!(f, "TIMEWAIT"),
            TcpStatus::CloseWait => write!(f, "CLOSEWAIT"),
            TcpStatus::LastAck => write!(f, "LASTACK")
        };
    }
}