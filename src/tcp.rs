use crate::interests::Interests;
use crate::selector::{PollGroup, Selector};
use crate::token::Token;
use crate::{
    afd_create_helper_handle, interests_to_epoll, ws_get_base_socket, HasOverlappedIoCompleted,
    PollInfoBinding, SOCK_KNOWN_EPOLL_EVENTS,
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

#[derive(PartialEq)]
enum SockPollState {
    SOCK_POLL_IDLE,
    SOCK_POLL_PENDING,
    SOCK_POLL_CANCELLED,
}

struct State {
    base_sock: SOCKET,
    poll_group: Option<PollGroup>,
    user_events: u32,
    pending_events: u32,
    user_data: u64,
    update_enqueued: bool, //to note if this TcpStream is in selector's update_queue
    delete_pending: bool,
    poll_state: SockPollState,
    binding: PollInfoBinding,
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
                delete_pending: false,
                poll_state: SockPollState::SOCK_POLL_IDLE,
                binding: PollInfoBinding::new(),
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

        if 0 != (self.state.user_events & *SOCK_KNOWN_EPOLL_EVENTS & !self.state.pending_events) {
            self.request_update(selector);
        }
    }

    pub(crate) fn request_update(&self, selector: &Selector) {
        if !self.state.update_enqueued {
            selector.enqueue_update(*self);
            self.state.update_enqueued = true;
        }
    }

    fn cancel_poll(&mut self) -> io::Result<()> {
        assert!(self.state.poll_state == SockPollState::SOCK_POLL_PENDING);

        if !HasOverlappedIoCompleted(&self.state.binding.overlapped) {
            if let Some(poll_group) = self.state.poll_group {
                let ret = winapi::um::ioapiset::CancelIoEx(
                    poll_group.afd_helper_handle,
                    &mut self.state.binding.overlapped as *mut _,
                );
                if ret == 0 {
                    let err = io::Error::last_os_error();
                    if err.kind() != io::ErrorKind::NotFound {
                        //io::ErrorKind::NotFound is verified to be the same as ERROR_NOT_FOUND
                        ///use std::io;
                        ///
                        ///fn main() {
                        ///    // ERROR_FILE_NOT_FOUND
                        ///    // 2 (0x2)
                        ///    // The system cannot find the file specified.
                        ///    // From: https://docs.microsoft.com/en-us/windows/win32/debug/system-error-codes--0-499-
                        ///    let error = io::Error::from_raw_os_error(0x2);
                        ///    assert_eq!(error.kind(), io::ErrorKind::NotFound);
                        ///}
                        return Err(err);
                    }
                }
            } else {
                unreachable!();
            }
        }

        self.state.poll_state = SockPollState::SOCK_POLL_CANCELLED;
        self.state.pending_events = 0;
        Ok(())
    }

    pub(crate) fn update(&mut self, selector: &mut Selector) -> io::Result<()> {
        assert!(!self.state.delete_pending);

        match self.state.poll_state {
            SockPollState::SOCK_POLL_PENDING => {
                if 0 != (self.state.user_events
                    & *SOCK_KNOWN_EPOLL_EVENTS
                    & !self.state.pending_events)
                {
                    self.cancel_poll()
                } else {
                    Ok(())
                }
            }
            SockPollState::SOCK_POLL_CANCELLED => {}
            SockPollState::SOCK_POLL_IDLE => {}
        }
    }
}
