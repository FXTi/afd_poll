use crate::event::Event;
use crate::interests::Interests;
use crate::ready::Ready;
use crate::tcp::{SockPollState, State, TcpStream};
use crate::token::Token;
use crate::{
    afd_create_helper_handle, init, sock_afd_events_to_epoll_events, PollInfoBinding,
    AFD_POLL_INFO, AFD_POLL_LOCAL_CLOSE,
};
use crate::{
    EPOLLERR, EPOLLHUP, EPOLLIN, EPOLLMSG, EPOLLONESHOT, EPOLLOUT, EPOLLPRI, EPOLLRDBAND,
    EPOLLRDHUP, EPOLLRDNORM, EPOLLWRBAND, EPOLLWRNORM,
};
use miow::iocp::{CompletionPort, CompletionStatus};
use std::collections::VecDeque;
use std::io;
use std::os::windows::io::AsRawHandle;
use std::os::windows::io::FromRawHandle;
use std::ptr::null_mut;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use winapi::shared::minwindef::DWORD;
use winapi::shared::ntstatus::STATUS_CANCELLED;
use winapi::shared::winerror::WAIT_TIMEOUT;
use winapi::um::handleapi::{GetHandleInformation, INVALID_HANDLE_VALUE};
use winapi::um::minwinbase::OVERLAPPED;
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
    pub fn new(iocp: &HANDLE) -> io::Result<PollGroup> {
        afd_create_helper_handle(iocp).map(|achh| PollGroup {
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
            PollGroup::new(&self.iocp).map(|pg| {
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
    update_deque: VecDeque<AtomicPtr<State>>,
    //We still need delete_queue
    delete_queue: VecDeque<AtomicPtr<State>>,
}

struct SelectorInner {
    id: usize,
    port: CompletionPort,
    lock: Mutex<()>,
}

impl SelectorInner {
    fn new(id: usize, iocp: &CompletionPort) -> SelectorInner {
        SelectorInner {
            id,
            //identical to iocp
            port: unsafe { CompletionPort::from_raw_handle(iocp.as_raw_handle()) },
            lock: Mutex::new(()),
        }
    }
}

impl Selector {
    pub fn new() -> io::Result<Selector> {
        //Equal to epoll_create, which create port_state representing iocp port
        init()?;

        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed) + 1;

        CompletionPort::new(1).map(|port| Selector {
            inner: Arc::new(SelectorInner::new(id, &port)),
            poll_group_queue: PollGroupQueue::new(&port),
            poll_count: 0,
            update_deque: VecDeque::new(),
            delete_queue: VecDeque::new(),
        })
    }

    fn check_iocp(&mut self) -> io::Result<()> {
        let mut flag: DWORD = DWORD::default();

        match self.port().as_raw_handle() {
            INVALID_HANDLE_VALUE => Err(io::Error::last_os_error()),
            iocp_handle => {
                if 0 == unsafe { GetHandleInformation(iocp_handle, &mut flag as *mut _) } {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(())
                }
            }
        }
    }

    pub fn select(&mut self, events: &mut Events, timeout: Option<Duration>) -> io::Result<()> {
        //init() appear in four functions in epoll
        //They are just four critical functions, epoll_*
        init()?;

        events.clear();

        {
            //Enter critical section
            self.inner.lock.lock().unwrap();

            self.update_events()?;
            (self.poll_count) += 1;
            //Exit critical section
        }

        //GetQueuedCompletionStatusEx() called here
        let n = match self.inner.port.get_many(&mut events.statuses, timeout) {
            Ok(statuses) => statuses.len(),
            Err(ref e) if e.raw_os_error() == Some(WAIT_TIMEOUT as i32) => 0,
            Err(e) => return Err(e),
        };

        {
            //Enter critical section
            self.inner.lock.lock().unwrap();

            self.poll_count -= 1;

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

                //Correctness of this convertion is unclear.
                let mut socket = unsafe { &mut (*(status.overlapped() as *mut State)) };

                match self.feed_event(socket)? {
                    None => {}
                    Some(ev) => events.events.push(ev),
                }
            }

            self.update_if_polling()?;
            //Exit critical section
        }

        Ok(())
    }

    fn feed_event(&mut self, socket: &mut State) -> io::Result<Option<Event>> {
        let poll_info = &socket.poll_info;
        let mut epoll_events: u32 = 0;

        socket.poll_state = SockPollState::SOCK_POLL_IDLE;
        socket.pending_events = 0;

        if socket.delete_pending {
            socket.delete(self, false)?;
            return Ok(None);
        } else if socket.overlapped.Internal == STATUS_CANCELLED as _ {
        } else if socket.overlapped.Internal < 0 {
            epoll_events = EPOLLERR;
        } else if socket.poll_info.NumberOfHandles < 1 {
        } else if socket.poll_info.Handles[0].Events & AFD_POLL_LOCAL_CLOSE != 0 {
            socket.delete(self, false)?;
            return Ok(None);
        } else {
            epoll_events = sock_afd_events_to_epoll_events(&socket.poll_info.Handles[0].Events);
        }

        socket.request_update(self);

        epoll_events &= socket.user_events;

        match epoll_events {
            0 => Ok(None),
            _ => {
                if socket.user_events & EPOLLONESHOT != 0 {
                    socket.user_events = 0;
                }

                Ok(Some(Event::new(
                    Ready::from_usize(epoll_events as _),
                    Token::from(socket.user_data as usize),
                )))
            }
        }
    }

    pub fn port(&self) -> &CompletionPort {
        &self.inner.port
    }

    pub(crate) fn enqueue_update(&mut self, tcp_stream: &mut State) {
        let element = AtomicPtr::new(&mut *tcp_stream as *mut _);
        self.update_deque.push_back(element);
    }

    pub(crate) fn enqueue_delete(&mut self, tcp_stream: &mut State) {
        let element = AtomicPtr::new(&mut *tcp_stream as *mut _);
        self.delete_queue.push_back(element);
    }

    pub(crate) fn dequeue_update(&mut self, tcp_stream: &mut State) {}

    pub(crate) fn dequeue_delete(&mut self, tcp_stream: &mut State) {}

    pub(crate) fn release_poll_group(&mut self, poll_group: &PollGroup) {}

    fn update_events(&mut self) -> io::Result<()> {
        while let Some(sock) = self.update_deque.pop_front() {
            unsafe { (*sock.load(Ordering::Relaxed)).update(self)? };
        }

        Ok(())
    }

    pub fn update_if_polling(&mut self) -> io::Result<()> {
        if self.poll_count > 0 {
            self.update_events()?;
        }

        Ok(())
    }

    pub fn register(
        &mut self,
        sock: &mut TcpStream,
        token: Token,
        interests: Interests,
    ) -> io::Result<()> {
        //embed register on selector by now
        //maybe move to struct which construct TcpStream in future pr
        init()?;

        let socket = sock.socket();

        let ws_base_socket = sock.base_socket().unwrap();

        sock.set_poll_group(self.poll_group_queue.acquire().unwrap());

        sock.set_events(interests, token, self);

        self.update_if_polling()
    }
}
#[derive(Debug)]
pub struct Events {
    /// Raw I/O event completions are filled in here by the call to `get_many`
    /// on the completion port above. These are then processed to run callbacks
    /// which figure out what to do after the event is done.
    statuses: Box<[CompletionStatus]>,

    /// Literal events returned by `get` to the upwards `EventLoop`. This file
    /// doesn't really modify this (except for the waker), instead almost all
    /// events are filled in by the `ReadinessQueue` from the `poll` module.
    events: Vec<Event>,
}

impl Events {
    pub fn with_capacity(cap: usize) -> Events {
        // Note that it's possible for the output `events` to grow beyond the
        // capacity as it can also include deferred events, but that's certainly
        // not the end of the world!
        Events {
            statuses: vec![CompletionStatus::zero(); cap].into_boxed_slice(),
            events: Vec::with_capacity(cap),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn capacity(&self) -> usize {
        self.events.capacity()
    }

    pub fn get(&self, idx: usize) -> Option<&Event> {
        self.events.get(idx)
    }

    pub fn push_event(&mut self, event: Event) {
        self.events.push(event);
    }

    pub fn clear(&mut self) {
        self.events.truncate(0);
    }
}
