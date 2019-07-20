use crate::interests::Interests;
use crate::selector::{PollGroup, Selector};
use crate::token::Token;
use crate::{
    afd_create_helper_handle, interests_to_epoll, ws_get_base_socket, SOCK__KNOWN_EPOLL_EVENTS,
};
use crate::{
    EPOLLERR, EPOLLHUP, EPOLLIN, EPOLLMSG, EPOLLONESHOT, EPOLLOUT, EPOLLPRI, EPOLLRDBAND,
    EPOLLRDHUP, EPOLLRDNORM, EPOLLWRBAND, EPOLLWRNORM,
};
use miow::iocp::CompletionPort;
use std::io;
use std::net;
use std::os::windows::io::AsRawHandle;
use std::os::windows::io::AsRawSocket;
use std::ptr::null_mut;
use std::sync::Arc;
use winapi::um::winnt::HANDLE;
use winapi::um::winsock2::SOCKET;

struct State {
    base_sock: SOCKET,
    poll_group: Option<PollGroup>,
    user_events: u32,
    pending_events: u32,
    user_data: u64,
    update_enqueued: bool,
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
                poll_group: None,
                user_events: 0,
                pending_events: 0,
                user_data: 0,
                update_enqueued: false,
            }),
        }
    }

    pub(crate) fn socket(&self) -> SOCKET {
        self.sock.as_raw_socket() as SOCKET
    }

    pub(crate) fn base_socket(&self) -> io::Result<SOCKET> {
        ws_get_base_socket(&self.socket()).map(|base_socket| {
            self.state.base_sock = base_socket;
            base_socket
        })
    }

    pub(crate) fn set_poll_group(&self, poll_group: PollGroup) {
        self.state.poll_group = Some(poll_group);
    }

    pub(crate) fn set_events(&self, interests: Interests, token: Token, selector: &Selector) {
        self.state.user_events = interests_to_epoll(interests) | EPOLLERR | EPOLLHUP;
        self.state.user_data = usize::from(token) as u64;

        if 0 != self.state.user_events & *SOCK__KNOWN_EPOLL_EVENTS & !self.state.pending_events {
            self.request_update(selector);
        }
    }

    pub(crate) fn request_update(&self, selector: &Selector) {
        if !self.state.update_enqueued {
            selector.enqueue_update(*self);
            self.state.update_enqueued = true;
        }
    }

    pub(crate) fn update(&mut self, selector: &mut Selector) -> io::Result<()> {
        Ok(())
    }
}
