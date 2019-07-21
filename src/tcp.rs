use crate::interests::Interests;
use crate::selector::{PollGroup, Selector};
use crate::token::Token;
use crate::{
    afd_create_helper_handle, afd_poll, interests_to_epoll, sock_epoll_events_to_afd_events,
    ws_get_base_socket, HasOverlappedIoCompleted, PollInfoBinding, AFD_POLL_HANDLE_INFO,
    AFD_POLL_INFO, SOCK_KNOWN_EPOLL_EVENTS,
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
use winapi::shared::minwindef::DWORD;
use winapi::shared::ntdef::NULL;
use winapi::shared::winerror::{ERROR_INVALID_HANDLE, ERROR_IO_PENDING};
use winapi::um::minwinbase::OVERLAPPED;
use winapi::um::winnt::HANDLE;
use winapi::um::winnt::LARGE_INTEGER;
use winapi::um::winsock2::SOCKET;

#[derive(PartialEq)]
pub(crate) enum SockPollState {
    SOCK_POLL_IDLE,
    SOCK_POLL_PENDING,
    SOCK_POLL_CANCELLED,
}

#[repr(C)]
pub(crate) struct State {
    pub overlapped: OVERLAPPED,
    pub poll_info: AFD_POLL_INFO,
    pub base_sock: SOCKET,
    pub poll_group: Option<PollGroup>,
    pub user_events: u32,
    pub pending_events: u32,
    pub user_data: u64,
    pub update_enqueued: bool, //to note if this TcpStream is in selector's update_queue
    pub delete_pending: bool,
    pub poll_state: SockPollState,
}

pub struct TcpStream {
    sock: net::TcpStream,
    state: State,
}

impl TcpStream {
    fn new(socket: net::TcpStream) -> TcpStream {
        TcpStream {
            sock: socket,
            state: State {
                base_sock: 0,
                poll_group: None,
                user_events: 0,
                pending_events: 0,
                user_data: 0,
                update_enqueued: false,
                delete_pending: false,
                poll_state: SockPollState::SOCK_POLL_IDLE,
                overlapped: OVERLAPPED::default(),
                poll_info: AFD_POLL_INFO {
                    Timeout: LARGE_INTEGER::default(),
                    NumberOfHandles: 1,
                    Exclusive: 0,
                    Handles: [AFD_POLL_HANDLE_INFO {
                        Handle: NULL,
                        Events: DWORD::default(),
                        Status: 0,
                    }],
                },
            },
        }
    }

    pub(crate) fn socket(&self) -> SOCKET {
        self.sock.as_raw_socket() as SOCKET
    }

    pub(crate) fn base_socket(&mut self) -> io::Result<SOCKET> {
        ws_get_base_socket(&self.socket()).map(|base_socket| {
            self.state.base_sock = base_socket;
            base_socket
        })
    }

    pub(crate) fn set_poll_group(&mut self, poll_group: PollGroup) {
        self.state.poll_group = Some(poll_group);
    }

    pub(crate) fn set_events(
        &mut self,
        interests: Interests,
        token: Token,
        selector: &mut Selector,
    ) {
        self.state.user_events = interests_to_epoll(interests) | EPOLLERR | EPOLLHUP;
        self.state.user_data = usize::from(token) as u64;

        if 0 != (self.state.user_events & *SOCK_KNOWN_EPOLL_EVENTS & !self.state.pending_events) {
            self.request_update(selector);
        }
    }

    pub(crate) fn request_update(&mut self, selector: &mut Selector) {
        self.state.request_update(selector)
    }

    fn cancel_poll(&mut self) -> io::Result<()> {
        self.state.cancel_poll()
    }

    pub(crate) fn delete(&mut self, selector: &mut Selector, force: bool) -> io::Result<()> {
        self.state.delete(selector, force)
    }

    pub(crate) fn update(&mut self, selector: &mut Selector) -> io::Result<()> {
        self.state.update(selector)
    }
}

impl State {
    pub(crate) fn request_update(&mut self, selector: &mut Selector) {
        if !self.update_enqueued {
            selector.enqueue_update(&mut *self);
            self.update_enqueued = true;
        }
    }

    fn cancel_poll(&mut self) -> io::Result<()> {
        assert!(self.poll_state == SockPollState::SOCK_POLL_PENDING);

        if !HasOverlappedIoCompleted(&self.overlapped) {
            if let Some(ref poll_group) = self.poll_group {
                let ret = unsafe {
                    winapi::um::ioapiset::CancelIoEx(
                        poll_group.afd_helper_handle,
                        &mut self.overlapped as *mut _,
                    )
                };
                match ret {
                    0 if io::Error::last_os_error().kind() == io::ErrorKind::NotFound => {}
                    0 => {
                        return Err(io::Error::last_os_error());
                    }
                    _ => {}
                }
            } else {
                unreachable!();
            }
        }

        self.poll_state = SockPollState::SOCK_POLL_CANCELLED;
        self.pending_events = 0;
        Ok(())
    }

    pub(crate) fn delete(&mut self, selector: &mut Selector, force: bool) -> io::Result<()> {
        if !self.delete_pending {
            if self.poll_state == SockPollState::SOCK_POLL_PENDING {
                self.cancel_poll()?;
            }
            //get this TcpStream off Selector's update_queue
            selector.dequeue_update(&mut *self);

            self.delete_pending = true;
        }

        if force || self.poll_state == SockPollState::SOCK_POLL_IDLE {
            selector.dequeue_delete(&mut *self);

            if let Some(ref pg) = self.poll_group {
                selector.release_poll_group(pg);
            } else {
                unreachable!();
            }
        //And then, free this TcpStream
        } else {
            selector.enqueue_delete(&mut *self);
        }

        Ok(())
    }

    pub(crate) fn update(&mut self, selector: &mut Selector) -> io::Result<()> {
        assert!(!self.delete_pending);

        match self.poll_state {
            SockPollState::SOCK_POLL_PENDING => {
                if 0 != (self.user_events & *SOCK_KNOWN_EPOLL_EVENTS & !self.pending_events) {
                    self.cancel_poll()
                } else {
                    Ok(())
                }
            }
            SockPollState::SOCK_POLL_CANCELLED => Ok(()),
            SockPollState::SOCK_POLL_IDLE => {
                //Start a new poll operation
                self.overlapped = OVERLAPPED::default();
                self.poll_info = AFD_POLL_INFO {
                    Timeout: LARGE_INTEGER::default(),
                    NumberOfHandles: 1,
                    Exclusive: 0,
                    Handles: [AFD_POLL_HANDLE_INFO {
                        Handle: self.base_sock as HANDLE,
                        Events: sock_epoll_events_to_afd_events(self.user_events),
                        Status: 0,
                    }],
                };
                unsafe { *self.poll_info.Timeout.QuadPart_mut() = i64::max_value() };

                if let Some(ref poll_group) = self.poll_group {
                    let r = afd_poll(
                        poll_group.afd_helper_handle,
                        &mut self.poll_info,
                        &mut self.overlapped,
                    );

                    match r {
                        Ok(()) => Ok(()),
                        Err(ref e) if e.raw_os_error() == Some(ERROR_INVALID_HANDLE as _) => {
                            self.delete(selector, false)
                        }
                        Err(ref e) if e.raw_os_error() == Some(ERROR_IO_PENDING as _) => Ok(()),
                        Err(e) => Err(e),
                    }
                } else {
                    unreachable!();
                }
            }
        }
    }
}
