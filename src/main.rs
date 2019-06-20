#[macro_use]
extern crate lazy_static;
use ntapi::ntioapi::{
    IO_STATUS_BLOCK_u, NtCreateFile, NtDeviceIoControlFile, FILE_OPEN, IO_STATUS_BLOCK,
};
use ntapi::ntrtl::RtlNtStatusToDosError;
use std::mem::size_of;
use winapi::shared::minwindef::{DWORD, LPVOID, ULONG, USHORT};
//use winapi::shared::ntdef::UNICODE_STRING;
use winapi::shared::ntdef::{NTSTATUS, OBJECT_ATTRIBUTES, PHANDLE, PVOID, PWCH};
use winapi::shared::ntstatus::{STATUS_PENDING, STATUS_SUCCESS};
use winapi::shared::winerror::WSAEINPROGRESS;
use winapi::um::handleapi::CloseHandle;
use winapi::um::ioapiset::CreateIoCompletionPort;
use winapi::um::minwinbase::OVERLAPPED;
use winapi::um::winbase::SetFileCompletionNotificationModes;
use winapi::um::winbase::FILE_SKIP_SET_EVENT_ON_HANDLE;
use winapi::um::winnt::{FILE_SHARE_READ, FILE_SHARE_WRITE, HANDLE, LARGE_INTEGER, SYNCHRONIZE};
use winapi::um::winsock2::{WSAIoctl, INVALID_SOCKET, SOCKET, SOCKET_ERROR};

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
    let mut piosb = overlapped.Internal as *mut IO_STATUS_BLOCK;

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
                0 as *mut _,
                0,
                &mut base_socket as *mut _ as LPVOID,
                size_of::<SOCKET>() as DWORD,
                &mut bytes as *mut _,
                0 as *mut _,
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

lazy_static! {
    static ref afd___helper_name: &'static str = "\\Device\\Afd\\Wepoll";
    static ref afd__helper_name: UNICODE_STRING = UNICODE_STRING {
        Length: afd___helper_name.len() as USHORT,
        MaximumLength: afd___helper_name.len() as USHORT,
        Buffer: afd___helper_name.as_ptr() as *const _ as *mut _,
    };
    static ref afd__helper_attributes: OBJECT_ATTRIBUTES = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as ULONG,
        RootDirectory: 0 as *mut _,
        ObjectName: &mut afd__helper_name as *mut _,
        Attributes: 0,
        SecurityDescriptor: 0 as *mut _,
        SecurityQualityOfService: 0 as *mut _,
    };
}

#[allow(non_snake_case)]
fn afd_create_helper_handle(iocp: &mut HANDLE, afd_helper_handle_out: &mut HANDLE) -> i32 {
    let mut afd_helper_handle: HANDLE = 0 as *mut _;
    let mut iosb = IO_STATUS_BLOCK {
        u: IO_STATUS_BLOCK_u { Status: 0 },
        Information: 0,
    };

    let status = unsafe {
        NtCreateFile(
            &mut afd_helper_handle as PHANDLE,
            SYNCHRONIZE,
            &mut afd__helper_attributes as *mut _,
            &mut iosb as *mut _,
            0 as *mut _,
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            FILE_OPEN,
            0,
            0 as *mut _,
            0,
        )
    };

    if status == STATUS_SUCCESS {
        return -1;
    }

    unsafe {
        if (0 as *mut _ == CreateIoCompletionPort(afd_helper_handle, *iocp, 0, 0))
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

fn main() {
    println!("Hello, world!");
}
