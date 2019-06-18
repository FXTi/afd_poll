use miow::Overlapped;
use ntapi::ntioapi::{IO_STATUS_BLOCK_u, NtDeviceIoControlFile, IO_STATUS_BLOCK};
use std::mem::size_of;
use winapi::shared::minwindef::{DWORD, ULONG};
use winapi::shared::ntdef::{NTSTATUS, PVOID};
use winapi::um::winnt::{HANDLE, LARGE_INTEGER};
use winapi::um::winsock2::{WSAIoctl, SOCKET};

#[repr(C)]
struct AFD_POLL_HANDLE_INFO {
    Handle: HANDLE,
    Events: ULONG,
    Status: NTSTATUS,
}

#[repr(C)]
struct AFD_POLL_INFO {
    Timeout: LARGE_INTEGER,
    NumberOfHandles: ULONG,
    Exclusive: ULONG,
    Handles: [AFD_POLL_HANDLE_INFO; 1],
}

fn afd_poll(
    afd_helper_handle: HANDLE,
    poll_info: &mut AFD_POLL_INFO,
    overlapped: &Overlapped,
) -> i32 {
    let mut iosb = IO_STATUS_BLOCK {
        u: IO_STATUS_BLOCK_u {
            Status: winapi::shared::ntstatus::STATUS_PENDING,
        },
        Information: 0,
    };

    unsafe {
        NtDeviceIoControlFile(
            afd_helper_handle,
            0 as *mut _,
            None,
            overlapped.raw() as *mut _,
            &mut iosb as *mut IO_STATUS_BLOCK,
            0x00012024,
            &mut *poll_info as *mut _ as PVOID,
            size_of::<AFD_POLL_INFO>() as u32,
            &mut *poll_info as *mut _ as PVOID,
            size_of::<AFD_POLL_INFO>() as u32,
        );
    }

    0
}

fn ws_get_base_socket(socket: &SOCKET) -> SOCKET {
    let mut base_socket: SOCKET = 0;
    let mut bytes: DWORD = 0;

    unsafe {
        WSAIoctl(
            socket,
            0x48000022,
            0 as *mut _,
            0,
            &mut base_socket as *mut _,
            size_of::<SOCKET>() as DWORD,
            &mut bytes as *mut _,
            0 as *mut _,
            None,
        );
    }
}

fn main() {
    println!("Hello, world!");
}
