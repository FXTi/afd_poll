use crate::interests::Interests;
use crate::tcp::TcpStream;
use crate::token::Token;
use crate::{init, sock_afd_events_to_epoll_events, PollInfoBinding};
use crate::{
    EPOLLERR, EPOLLHUP, EPOLLIN, EPOLLMSG, EPOLLONESHOT, EPOLLOUT, EPOLLPRI, EPOLLRDBAND,
    EPOLLRDHUP, EPOLLRDNORM, EPOLLWRBAND, EPOLLWRNORM,
};
use miow::iocp::{CompletionPort, CompletionStatus};
use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use winapi::shared::winerror::WAIT_TIMEOUT;

static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

pub struct Selector {
    inner: Arc<SelectorInner>,
}

struct SelectorInner {
    id: usize,
    port: CompletionPort,
}

impl Selector {
    pub fn new() -> io::Result<Selector> {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed) + 1;

        CompletionPort::new(1).map(|port| Selector {
            inner: Arc::new(SelectorInner { id, port }),
        })
    }

    pub fn select(
        &self,
        events: &mut Events,
        awakener: Token,
        timeout: Option<Duration>,
    ) -> io::Result<bool> {
        events.clear();

        //According to wepoll, events.status here should be a array of 256 elements.
        let n = match self.inner.port.get_many(&mut events.statuses, timeout) {
            Ok(statuses) => statuses.len(),
            Err(ref e) if e.raw_os_error() == Some(WAIT_TIMEOUT as i32) => 0,
            Err(e) => return Err(e),
        };

        let mut ret = false;
        for status in events.statuses[..n].iter() {
            /*
            // This should only ever happen from the awakener, and we should
            // only ever have one awakener right now, so assert as such.
            if status.overlapped() as usize == 0 {
                assert_eq!(status.token(), usize::from(awakener));
                ret = true;
                continue;
            }
            */

            //Extract events from lpOverlapped, problem is the lifetime of PollInfoBinding passed to underlaying API.
            let afd_poll_info = &(*(status.overlapped() as *const PollInfoBinding)).poll_info;
            let iocp_events = sock_afd_events_to_epoll_events(&afd_poll_info.Handles[0].Events);
        }

        Ok(ret)
    }

    pub fn port(&self) -> &CompletionPort {
        &self.inner.port
    }

    pub fn register(
        &mut self,
        sock: &TcpStream,
        token: Token,
        interests: Interests,
    ) -> io::Result<()> {
        init().unwrap();

        let socket = sock.socket();
        //Then set interests, considering convert from Interests to undrlaying type
        let socket_event: u32 = EPOLLERR | EPOLLHUP | EPOLLIN | EPOLLOUT;

        sock.afd_helper_handle(&mut self.inner.port);

        //update queue??
        Ok(())
    }
}
