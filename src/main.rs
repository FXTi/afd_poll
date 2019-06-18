use ntapi::ntioapi::{NtDeviceIoControlFile, IO_STATUS_BLOCK};
use ntapi::ntrtl::RtlNtStatusToDosError;
use std::mem::size_of;
use winapi::shared::minwindef::{DWORD, LPVOID, ULONG};
use winapi::shared::ntdef::{NTSTATUS, PVOID};
use winapi::shared::ntstatus::{STATUS_PENDING, STATUS_SUCCESS};
use winapi::shared::winerror::WSAEINPROGRESS;
use winapi::um::minwinbase::OVERLAPPED;
use winapi::um::winnt::{HANDLE, LARGE_INTEGER};
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

fn main() {
    println!("Hello, world!");
}
