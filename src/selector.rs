use crate::interests::Interests;
use crate::tcp::TcpStream;
use crate::token::Token;
use crate::{afd_create_helper_handle, init, sock_afd_events_to_epoll_events, PollInfoBinding};
use crate::{
    EPOLLERR, EPOLLHUP, EPOLLIN, EPOLLMSG, EPOLLONESHOT, EPOLLOUT, EPOLLPRI, EPOLLRDBAND,
    EPOLLRDHUP, EPOLLRDNORM, EPOLLWRBAND, EPOLLWRNORM,
};
use miow::iocp::{CompletionPort, CompletionStatus};
use std::collections::VecDeque;
use std::io;
use std::os::windows::io::AsRawHandle;
use std::ptr::null_mut;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use winapi::shared::winerror::WAIT_TIMEOUT;
use winapi::um::winnt::HANDLE;

static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
static MAX_SOCKET_PER_POLL_GROUP: i32 = 32;

#[derive(Clone)]
pub struct PollGroup {
    pub group_size: i32,
    //one PollGroup can have at most 32 socket associated
    pub afd_helper_handle: HANDLE,
}

impl PollGroup {
    pub fn new(iocp: HANDLE) -> io::Result<PollGroup> {
        afd_create_helper_handle(&mut iocp).map(|achh| PollGroup {
            group_size: 0,
            afd_helper_handle: achh,
        })
    }
}

pub struct PollGroupQueue {
    queue: Vec<PollGroup>,
    iocp: HANDLE,
}

impl PollGroupQueue {
    pub fn new(completion_port: &CompletionPort) -> PollGroupQueue {
        PollGroupQueue {
            queue: Vec::new(),
            iocp: completion_port.as_raw_handle(),
        }
    }

    pub fn acquire(&mut self) -> io::Result<PollGroup> {
        let n = self.queue.len();
        if n == 0 || self.queue[n - 1].group_size > MAX_SOCKET_PER_POLL_GROUP {
            PollGroup::new(self.iocp).map(|pg| {
                self.queue.push(pg);
            });
        }
        self.queue[n - 1].group_size += 1;
        Ok(self.queue[n - 1].clone())
    }
}

pub struct Selector {
    inner: Arc<SelectorInner>,
    //act as poll_group in wepoll, to manage limited use of afd_helper_handle
    poll_group_queue: PollGroupQueue,
    //to note the number of thread who is polling on this iocp port
    poll_count: i32,
    //We still need update_queue
    update_deque: VecDeque<AtomicPtr<TcpStream>>,
}

struct SelectorInner {
    id: usize,
    port: CompletionPort,
    lock: Mutex<()>,
}

impl Selector {
    pub fn new() -> io::Result<Selector> {
        //Equal to epoll_create, which create port_state representing iocp port
        init().unwrap();

        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed) + 1;

        CompletionPort::new(1).map(|port| Selector {
            inner: Arc::new(SelectorInner {
                id,
                port,
                lock: Mutex::new(()),
            }),
            poll_group_queue: PollGroupQueue::new(&port),
            poll_count: 0,
            update_deque: VecDeque::new(),
        })
    }

    pub fn select(
        &self,
        events: &mut Events,
        awakener: Token,
        timeout: Option<Duration>,
    ) -> io::Result<bool> {
        //init() appear in four functions in epoll
        //They are just four critical functions, epoll_*
        init().unwrap();

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

    pub(crate) fn enqueue_update(&mut self, tcp_stream: TcpStream) {
        let element = AtomicPtr::new(&mut tcp_stream as *mut _);
        self.update_deque.push_back(element);
    }

    pub fn update_if_polling(&mut self) -> io::Result<()> {
        if self.poll_count > 0 {
            while let Some(sock) = self.update_deque.pop_front() {
                (*sock.load(Ordering::Relaxed)).update(self);
            }
        }

        Ok(())
    }

    pub fn register(
        &mut self,
        sock: &TcpStream,
        token: Token,
        interests: Interests,
    ) -> io::Result<()> {
        //embed register on selector by now
        //maybe move to struct which construct TcpStream in future pr
        init().unwrap();

        let socket = sock.socket();

        let ws_base_socket = sock.base_socket().unwrap();

        sock.set_poll_group(self.poll_group_queue.acquire().unwrap());

        sock.set_events(interests, token, self);

        self.update_if_polling()
    }
}
