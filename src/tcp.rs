use crate::{afd_create_helper_handle, ws_get_base_socket};
use miow::iocp::CompletionPort;
use std::net;
use std::os::windows::io::AsRawHandle;
use std::os::windows::io::AsRawSocket;
use std::ptr::null_mut;
use std::sync::Arc;
use winapi::um::winnt::HANDLE;
use winapi::um::winsock2::SOCKET;

struct State {
    base_sock: SOCKET,
    afd_helper_handle: HANDLE,
}

pub struct TcpStream {
    sock: net::TcpStream,
    state: Arc<State>,
}

impl TcpStream {
    fn new(socket: net::TcpStream) -> TcpStream {
        TcpStream {
            sock: socket,
            state: Arc::new(State {
                base_sock: 0,
                afd_helper_handle: null_mut(),
            }),
        }
    }

    pub(crate) fn socket(&self) -> SOCKET {
        self.sock.as_raw_socket() as SOCKET
    }

    pub(crate) fn base_socket(&mut self) -> SOCKET {
        //init base_socket and return
        self.state.base_sock = ws_get_base_socket(&self.socket());

        self.state.base_sock
    }

    pub(crate) fn afd_helper_handle(&mut self, iocp: &mut CompletionPort) -> HANDLE {
        //Whether use a poll group?
        let mut out: HANDLE = null_mut();
        afd_create_helper_handle(&mut iocp.as_raw_handle(), &mut out);

        out
    }
}
