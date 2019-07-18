mod interests;
mod selector;
mod tcp;
mod token;
#[macro_use]
extern crate lazy_static;
use crate::interests::Interests;
use ntapi::ntioapi::{
    IO_STATUS_BLOCK_u, NtCreateFile, NtDeviceIoControlFile, FILE_OPEN, IO_STATUS_BLOCK,
};
use ntapi::ntrtl::RtlNtStatusToDosError;
use std::io;
use std::mem::size_of;
use widestring::U16CString;
use winapi::shared::minwindef::{DWORD, FALSE, LPVOID, MAKEWORD, ULONG, USHORT};
//use winapi::shared::ntdef::UNICODE_STRING;
//use winapi::shared::ntdef::OBJECT_ATTRIBUTES;
use libc::EPOLLET;
use std::net::{TcpListener, TcpStream};
use std::os::windows::io::AsRawSocket;
use std::{cmp, thread, time};
use winapi::shared::ntdef::{NTSTATUS, NULL, PHANDLE, PUNICODE_STRING, PVOID, PWCH};
use winapi::shared::ntstatus::{STATUS_PENDING, STATUS_SUCCESS};
use winapi::shared::winerror::WSAEINPROGRESS;
use winapi::shared::ws2def::WSABUF;
use winapi::um::handleapi::{CloseHandle, INVALID_HANDLE_VALUE};
use winapi::um::ioapiset::{CreateIoCompletionPort, GetQueuedCompletionStatusEx};
use winapi::um::minwinbase::{OVERLAPPED, OVERLAPPED_ENTRY};
use winapi::um::winbase::{
    SetFileCompletionNotificationModes, FILE_SKIP_SET_EVENT_ON_HANDLE, INFINITE,
};
use winapi::um::winnt::{FILE_SHARE_READ, FILE_SHARE_WRITE, HANDLE, LARGE_INTEGER, SYNCHRONIZE};
use winapi::um::winsock2::{u_long, WSARecv};
use winapi::um::winsock2::{WSAIoctl, WSAStartup, INVALID_SOCKET, SOCKET, SOCKET_ERROR, WSADATA};

#[allow(non_snake_case)]
#[repr(C)]
struct AFD_POLL_HANDLE_INFO {
    Handle: HANDLE,
    Events: ULONG,
    Status: NTSTATUS,
}

#[allow(non_snake_case)]
#[repr(C)]
struct AFD_POLL_INFO {
    Timeout: LARGE_INTEGER,
    NumberOfHandles: ULONG,
    Exclusive: ULONG,
    Handles: [AFD_POLL_HANDLE_INFO; 1],
}

const IOCTL_AFD_POLL: ULONG = 0x00012024;

fn afd_poll(
    afd_helper_handle: HANDLE,
    poll_info: &mut AFD_POLL_INFO,
    overlapped: &mut OVERLAPPED,
) -> u32 {
    let mut piosb = &mut overlapped.Internal as *mut _ as *mut IO_STATUS_BLOCK;

    let status = unsafe {
        (*piosb).u.Status = STATUS_PENDING;

        NtDeviceIoControlFile(
            afd_helper_handle,
            overlapped.hEvent,
            None,
            &mut *overlapped as *mut _ as PVOID,
            piosb,
            IOCTL_AFD_POLL,
            &mut *poll_info as *mut _ as PVOID,
            size_of::<AFD_POLL_INFO>() as u32,
            &mut *poll_info as *mut _ as PVOID,
            size_of::<AFD_POLL_INFO>() as u32,
        )
    };

    match status {
        STATUS_SUCCESS => 0,
        STATUS_PENDING => WSAEINPROGRESS,
        _ => unsafe { RtlNtStatusToDosError(status) },
    }
}

const SIO_BASE_HANDLE: DWORD = 0x48000022;

fn ws_get_base_socket(socket: &SOCKET) -> io::Result<SOCKET> {
    let mut base_socket: SOCKET = 0;
    let mut bytes: DWORD = 0;

    unsafe {
        if SOCKET_ERROR
            == WSAIoctl(
                *socket,
                SIO_BASE_HANDLE,
                NULL,
                0,
                &mut base_socket as *mut _ as LPVOID,
                size_of::<SOCKET>() as DWORD,
                &mut bytes as *mut _,
                NULL as _,
                None,
            )
        {
            return Err(io::Error::new(io::ErrorKind::Other, "INVALID_SOCKET"));
        }
    }

    Ok(base_socket)
}

#[allow(non_snake_case)]
#[repr(C)]
struct UNICODE_STRING {
    Length: USHORT,
    MaximumLength: USHORT,
    Buffer: PWCH,
}

unsafe impl Send for UNICODE_STRING {}
unsafe impl Sync for UNICODE_STRING {}

#[allow(non_snake_case)]
#[repr(C)]
struct OBJECT_ATTRIBUTES {
    Length: ULONG,
    RootDirectory: HANDLE,
    ObjectName: PUNICODE_STRING,
    Attributes: ULONG,
    SecurityDescriptor: PVOID,
    SecurityQualityOfService: PVOID,
}

unsafe impl Send for OBJECT_ATTRIBUTES {}
unsafe impl Sync for OBJECT_ATTRIBUTES {}

lazy_static! {
    static ref afd___helper_name: U16CString =
        U16CString::from_str("\\Device\\Afd\\Wepoll").unwrap();
    static ref afd___helper_name_len: usize = U16CString::from_str("\\Device\\Afd\\Wepoll")
        .unwrap()
        .into_vec_with_nul()
        .len()
        * size_of::<u16>();
    static ref afd__helper_name: UNICODE_STRING = UNICODE_STRING {
        Length: *afd___helper_name_len as USHORT,
        MaximumLength: (*afd___helper_name_len - size_of::<u16>()) as USHORT,
        Buffer: afd___helper_name.as_ptr() as *const _ as *mut _,
    };
    static ref afd__helper_attributes: OBJECT_ATTRIBUTES = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as ULONG,
        RootDirectory: NULL,
        ObjectName: &*afd__helper_name as *const _ as *mut _,
        Attributes: 0,
        SecurityDescriptor: NULL,
        SecurityQualityOfService: NULL,
    };
    static ref init_done: bool = false;
    static ref SOCK__KNOWN_EPOLL_EVENTS: u32 = EPOLLIN
        | EPOLLPRI
        | EPOLLOUT
        | EPOLLERR
        | EPOLLHUP
        | EPOLLRDNORM
        | EPOLLRDBAND
        | EPOLLWRNORM
        | EPOLLWRBAND
        | EPOLLMSG
        | EPOLLRDHUP;
}

#[allow(non_snake_case)]
fn afd_create_helper_handle(iocp: &mut HANDLE) -> io::Result<HANDLE> {
    let mut afd_helper_handle: HANDLE = NULL;
    let mut iosb = IO_STATUS_BLOCK {
        u: IO_STATUS_BLOCK_u { Status: 0 },
        Information: 0,
    };

    let status = unsafe {
        NtCreateFile(
            &mut afd_helper_handle as PHANDLE,
            SYNCHRONIZE,
            &*afd__helper_attributes as *const _ as *mut _,
            &mut iosb as *mut _,
            NULL as _,
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            FILE_OPEN,
            0,
            NULL,
            0,
        )
    };

    if status != STATUS_SUCCESS {
        println!("NtCreateFile error: 0x{:x?}", unsafe {
            RtlNtStatusToDosError(status)
        });
        return Err(io::Error::new(io::ErrorKind::Other, status.to_string()));
    }

    unsafe {
        if (NULL == CreateIoCompletionPort(afd_helper_handle, *iocp, 0, 0))
            || (0
                == SetFileCompletionNotificationModes(
                    afd_helper_handle,
                    FILE_SKIP_SET_EVENT_ON_HANDLE,
                ))
        {
            CloseHandle(afd_helper_handle);
            Err(io::Error::new(io::ErrorKind::Other, ""))
        } else {
            Ok(afd_helper_handle)
        }
    }
}

#[allow(non_snake_case)]
fn port__create_iocp() -> HANDLE {
    //just return the result, error handling left for future
    let iocp = unsafe { CreateIoCompletionPort(INVALID_HANDLE_VALUE, NULL, 0, 0) };

    iocp
}

fn ws_global_init() -> i32 {
    let mut wsa_data = WSADATA::default();

    let r = unsafe { WSAStartup(MAKEWORD(2, 2), &mut wsa_data as *mut _) };

    match r {
        0 => 0,
        _ => -1,
    }
}

fn init() -> io::Result<()> {
    if !*init_done {
        //Do WS's init for now
        if ws_global_init() < 0 {
            return Err(io::Error::last_os_error());
        }

        *init_done = true;
    }

    Ok(())
}

const AFD_POLL_RECEIVE: ULONG = 0x0001;
const AFD_POLL_RECEIVE_EXPEDITED: ULONG = 0x0002;
const AFD_POLL_SEND: ULONG = 0x0004;
const AFD_POLL_DISCONNECT: ULONG = 0x0008;
const AFD_POLL_ABORT: ULONG = 0x0010;
const AFD_POLL_LOCAL_CLOSE: ULONG = 0x0020;
const AFD_POLL_ACCEPT: ULONG = 0x0080;
const AFD_POLL_CONNECT_FAIL: ULONG = 0x0100;

const EPOLLIN: u32 = 0b1;
const EPOLLPRI: u32 = 0b10;
const EPOLLOUT: u32 = 0b100;
const EPOLLERR: u32 = 0b1000;
const EPOLLHUP: u32 = 0b10000;
const EPOLLRDNORM: u32 = 0b1000000;
const EPOLLRDBAND: u32 = 0b10000000;
const EPOLLWRNORM: u32 = 0b100000000;
const EPOLLWRBAND: u32 = 0b1000000000;
const EPOLLMSG: u32 = 0b10000000000;
const EPOLLRDHUP: u32 = 0b10000000000000;
const EPOLLONESHOT: u32 = 0b10000000000000000000000000000000;

fn sock_epoll_events_to_afd_events(epoll_events: u32) -> DWORD {
    /* Always monitor for AFD_POLL_LOCAL_CLOSE, which is triggered when the
     * socket is closed with closesocket() or CloseHandle(). */
    let mut afd_events = AFD_POLL_LOCAL_CLOSE;

    if 0 != (epoll_events & (EPOLLIN | EPOLLRDNORM)) {
        afd_events |= AFD_POLL_RECEIVE | AFD_POLL_ACCEPT;
    }
    if 0 != (epoll_events & (EPOLLPRI | EPOLLRDBAND)) {
        afd_events |= AFD_POLL_RECEIVE_EXPEDITED;
    }
    if 0 != (epoll_events & (EPOLLOUT | EPOLLWRNORM | EPOLLWRBAND)) {
        afd_events |= AFD_POLL_SEND;
    }
    if 0 != (epoll_events & (EPOLLIN | EPOLLRDNORM | EPOLLRDHUP)) {
        afd_events |= AFD_POLL_DISCONNECT;
    }
    if 0 != (epoll_events & EPOLLHUP) {
        afd_events |= AFD_POLL_ABORT;
    }
    if 0 != (epoll_events & EPOLLERR) {
        afd_events |= AFD_POLL_CONNECT_FAIL;
    }

    afd_events
}

fn sock_afd_events_to_epoll_events(afd_events: &DWORD) -> u32 {
    let mut epoll_events: u32 = 0;

    if 0 != (*afd_events & (AFD_POLL_RECEIVE | AFD_POLL_ACCEPT)) {
        epoll_events |= EPOLLIN | EPOLLRDNORM;
    }
    if 0 != (*afd_events & AFD_POLL_RECEIVE_EXPEDITED) {
        epoll_events |= EPOLLPRI | EPOLLRDBAND;
    }
    if 0 != (*afd_events & AFD_POLL_SEND) {
        epoll_events |= EPOLLOUT | EPOLLWRNORM | EPOLLWRBAND;
    }
    if 0 != (*afd_events & AFD_POLL_DISCONNECT) {
        epoll_events |= EPOLLIN | EPOLLRDNORM | EPOLLRDHUP;
    }
    if 0 != (*afd_events & AFD_POLL_ABORT) {
        epoll_events |= EPOLLHUP;
    }
    if 0 != (*afd_events & AFD_POLL_CONNECT_FAIL) {
        /* Linux reports all these events after connect() has failed. */
        epoll_events |= EPOLLIN | EPOLLOUT | EPOLLERR | EPOLLRDNORM | EPOLLWRNORM | EPOLLRDHUP;
    }

    epoll_events
}

unsafe fn slice2buf(slice: &[u8]) -> WSABUF {
    WSABUF {
        len: cmp::min(slice.len(), <u_long>::max_value() as usize) as u_long,
        buf: slice.as_ptr() as *mut _,
    }
}

#[repr(C)]
struct PollInfoBinding {
    overlapped: OVERLAPPED,
    poll_info: AFD_POLL_INFO,
}

fn interests_to_epoll(interests: Interests) -> u32 {
    //Will change EPOLLET later
    let mut kind = EPOLLET;

    if interests.is_readable() {
        kind |= EPOLLIN;
    }

    if interests.is_writable() {
        kind |= EPOLLOUT;
    }

    kind as u32
}

#[test]
fn test_tcp_listener() {
    //epoll_create() start
    assert_eq!(ws_global_init(), 0);

    let mut iocp: HANDLE = port__create_iocp();
    assert!(iocp != NULL);
    //epoll_create() end

    //create test socket
    //Spawn thread to connect to TcpListener
    thread::spawn(|| {
        let one_sec = time::Duration::from_secs(1);
        thread::sleep(one_sec);
        let stream = TcpStream::connect("127.0.0.1:12345").unwrap();
        thread::sleep(one_sec);
        stream
    });

    //Create listener
    let listener = TcpListener::bind("127.0.0.1:12345").unwrap();
    let (net_sock, _) = listener.accept().unwrap();
    let sock = net_sock.as_raw_socket() as SOCKET;
    std::mem::forget(listener);
    std::mem::forget(net_sock);
    let socket_event: u32 = EPOLLERR | EPOLLHUP | EPOLLIN | EPOLLOUT;

    //Is this needed?
    {
        let mut buff: [u8; 256] = [u8::default(); 256];
        let mut buf = unsafe { slice2buf(&buff) };
        let mut flags = 0;
        let mut bytes_read: DWORD = 0;
        let mut overlapped = OVERLAPPED::default();
        unsafe {
            WSARecv(
                sock,
                &mut buf,
                1,
                &mut bytes_read,
                &mut flags,
                &mut overlapped as *mut _,
                None,
            );
        }
    }

    //port__ctl_add() start
    let base_sock = ws_get_base_socket(&sock).unwrap();

    let mut afd_helper_handle = afd_create_helper_handle(&mut iocp).unwrap();
    println!("{:?}", afd_helper_handle);

    let mut binding = Box::new(PollInfoBinding {
        overlapped: OVERLAPPED::default(),
        poll_info: AFD_POLL_INFO {
            Timeout: LARGE_INTEGER::default(),
            NumberOfHandles: 1,
            Exclusive: 0,
            Handles: [AFD_POLL_HANDLE_INFO {
                Handle: base_sock as HANDLE,
                Events: sock_epoll_events_to_afd_events(socket_event),
                Status: 0,
            }],
        },
    });
    unsafe { *binding.poll_info.Timeout.QuadPart_mut() = i64::max_value() };
    //memset(&sock_state->overlapped, 0, sizeof sock_state->overlapped);

    let r = afd_poll(
        afd_helper_handle,
        &mut binding.poll_info,
        &mut binding.overlapped,
    );
    assert_eq!(r, 0);
    //port__ctl_add() end

    //epoll_wait start
    let mut completion_count: DWORD = 0;
    let mut iocp_events: [OVERLAPPED_ENTRY; 256] = [OVERLAPPED_ENTRY::default(); 256];
    let r = unsafe {
        GetQueuedCompletionStatusEx(
            iocp,
            iocp_events.as_mut_ptr(),
            iocp_events.len() as ULONG,
            &mut completion_count as *mut _,
            //INFINITE,
            //Just wait 3 second for testing
            3000,
            FALSE,
        )
    };
    //epoll_wait end

    //println!("Return value: {:?}", r);
    //println!("completion_count: {:?}", completion_count);
    //println!("iocp_events: ");
    assert_eq!(completion_count, 1);
    for ele in iocp_events[0..completion_count as usize].iter() {
        //println!("  Event: ");
        //println!("    lpCompletionKey: {:?}", ele.lpCompletionKey);
        //println!("    lpOverlapped: {:?}", ele.lpOverlapped);
        if NULL as *const OVERLAPPED != ele.lpOverlapped {
            unsafe {
                let afd_poll_info = &(*(ele.lpOverlapped as *const PollInfoBinding)).poll_info;
                let iocp_events = sock_afd_events_to_epoll_events(&afd_poll_info.Handles[0].Events);
                //println!("      events: 0x{:x?}", iocp_events);
                assert!(iocp_events & EPOLLOUT != 0);
            }
        }
        //println!("    Internal: {:?}", ele.Internal);
        //println!(
        //"    dwNumberOfBytesTransferred: {:?}",
        //ele.dwNumberOfBytesTransferred
        //);
    }
}
