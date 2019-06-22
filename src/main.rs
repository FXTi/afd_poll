#[macro_use]
extern crate lazy_static;
use ntapi::ntioapi::{
    IO_STATUS_BLOCK_u, NtCreateFile, NtDeviceIoControlFile, FILE_OPEN, IO_STATUS_BLOCK,
};
use ntapi::ntrtl::RtlNtStatusToDosError;
use std::mem::size_of;
use widestring::U16CString;
use winapi::shared::minwindef::{DWORD, FALSE, LPVOID, MAKEWORD, ULONG, USHORT};
//use winapi::shared::ntdef::UNICODE_STRING;
//use winapi::shared::ntdef::OBJECT_ATTRIBUTES;
use winapi::shared::ntdef::{NTSTATUS, NULL, PHANDLE, PUNICODE_STRING, PVOID, PWCH};
use winapi::shared::ntstatus::{STATUS_PENDING, STATUS_SUCCESS};
use winapi::shared::winerror::WSAEINPROGRESS;
use winapi::shared::ws2def::{AF_INET, IPPROTO_TCP, SOCK_STREAM};
use winapi::um::handleapi::{CloseHandle, INVALID_HANDLE_VALUE};
use winapi::um::ioapiset::{CreateIoCompletionPort, GetQueuedCompletionStatusEx};
use winapi::um::minwinbase::{OVERLAPPED, OVERLAPPED_ENTRY};
use winapi::um::winbase::FILE_SKIP_SET_EVENT_ON_HANDLE;
use winapi::um::winbase::{SetFileCompletionNotificationModes, INFINITE};
use winapi::um::winnt::{FILE_SHARE_READ, FILE_SHARE_WRITE, HANDLE, LARGE_INTEGER, SYNCHRONIZE};
use winapi::um::winsock2::{
    socket, WSAIoctl, WSAStartup, INVALID_SOCKET, SOCKET, SOCKET_ERROR, WSADATA,
};

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

fn ws_get_base_socket(socket: &SOCKET) -> SOCKET {
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
            return INVALID_SOCKET;
        }
    }

    base_socket
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
    static ref afd__helper_name: UNICODE_STRING = UNICODE_STRING {
        Length: (size_of::<afd___helper_name>() - size_of::<u16>()) as USHORT,
        MaximumLength: size_of::<afd___helper_name>() as USHORT,
        Buffer: afd___helper_name.as_ptr() as *const _ as *mut _,
    };
    static ref afd__helper_attributes: OBJECT_ATTRIBUTES = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as ULONG,
        RootDirectory: NULL,
        ObjectName: &afd__helper_name as *const _ as *mut _,
        Attributes: 0,
        SecurityDescriptor: NULL,
        SecurityQualityOfService: NULL,
    };
}

#[allow(non_snake_case)]
fn afd_create_helper_handle(iocp: &mut HANDLE, afd_helper_handle_out: &mut HANDLE) -> i32 {
    let mut afd_helper_handle: HANDLE = NULL;
    let mut iosb = IO_STATUS_BLOCK {
        u: IO_STATUS_BLOCK_u { Status: 0 },
        Information: 0,
    };

    let status = unsafe {
        NtCreateFile(
            &mut afd_helper_handle as PHANDLE,
            SYNCHRONIZE,
            &afd__helper_attributes as *const _ as *mut _,
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

    if status == STATUS_SUCCESS {
        return -1;
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
            -1
        } else {
            *afd_helper_handle_out = afd_helper_handle;
            0
        }
    }
}

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

const AFD_POLL_RECEIVE: ULONG = 0x0001;
const AFD_POLL_RECEIVE_EXPEDITED: ULONG = 0x0002;
const AFD_POLL_SEND: ULONG = 0x0004;
const AFD_POLL_DISCONNECT: ULONG = 0x0008;
const AFD_POLL_ABORT: ULONG = 0x0010;
const AFD_POLL_LOCAL_CLOSE: ULONG = 0x0020;
const AFD_POLL_ACCEPT: ULONG = 0x0080;
const AFD_POLL_CONNECT_FAIL: ULONG = 0x0100;

fn main() {
    let mut iocp: HANDLE = port__create_iocp();
    assert!(iocp != NULL);

    let mut afd_helper_handle: HANDLE = NULL;
    afd_create_helper_handle(&mut iocp, &mut afd_helper_handle);
    println!("{:?}", afd_helper_handle);

    ws_global_init();
    println!("WS init complete.");

    let sock = unsafe { socket(AF_INET, SOCK_STREAM, IPPROTO_TCP as i32) };
    let mut base_sock = ws_get_base_socket(&sock);

    let mut poll_info = AFD_POLL_INFO {
        Timeout: LARGE_INTEGER::default(),
        NumberOfHandles: 1,
        Exclusive: 0,
        Handles: [AFD_POLL_HANDLE_INFO {
            Handle: &mut base_sock as *mut _ as HANDLE,
            Events: AFD_POLL_RECEIVE | AFD_POLL_ACCEPT,
            Status: 0,
        }],
    };
    let mut overlapped = OVERLAPPED::default();
    unsafe { *poll_info.Timeout.QuadPart_mut() = i64::max_value() };
    //memset(&sock_state->overlapped, 0, sizeof sock_state->overlapped);

    let r = afd_poll(afd_helper_handle, &mut poll_info, &mut overlapped);
    println!("{:?}", r);

    let mut completion_count: DWORD = 0;
    let mut iocp_events: [OVERLAPPED_ENTRY; 256] = [OVERLAPPED_ENTRY::default(); 256];
    let r = unsafe {
        GetQueuedCompletionStatusEx(
            iocp,
            iocp_events.as_mut_ptr(),
            iocp_events.len() as ULONG,
            &mut completion_count as *mut _,
            INFINITE,
            FALSE,
        )
    };

    println!("Return value: {:?}", r);
    println!("completion_count: {:?}", completion_count);
    println!("iocp_events: ");
    for ele in iocp_events[0..completion_count as usize].iter() {
        println!("  Event: ");
        println!("    lpCompletionKey: {:?}", ele.lpCompletionKey);
        println!("    lpOverlapped: {:?}", ele.lpOverlapped);
        /* ignore it for now
        if NULL as _ != ele.lpOverlapped {
            println!("      *lpOverlapped: {:?}", *ele.lpOverlapped);
        }
        */
        println!("    Internal: {:?}", ele.Internal);
        println!(
            "    dwNumberOfBytesTransferred: {:?}",
            ele.dwNumberOfBytesTransferred
        );
    }
}
