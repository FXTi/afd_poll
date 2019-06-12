use miow::Overlapped;
use ntapi::ntioapi::{IO_STATUS_BLOCK_u, NtDeviceIoControlFile, IO_STATUS_BLOCK};
use std::mem::size_of;
use winapi::shared::minwindef::ULONG;
use winapi::shared::ntdef::NTSTATUS;
use winapi::um::winnt::{HANDLE, LARGE_INTEGER};

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

fn afd_poll(afd_helper_handle: HANDLE, poll_info: &mut AFD_POLL_INFO, overlapped: &Overlapped) -> i32 {
    let iosb = IO_STATUS_BLOCK {
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
            &mut *poll_info as *mut _,
            size_of::<AFD_POLL_INFO>() as u32,
            &mut *poll_info as *mut _,
            size_of::<AFD_POLL_INFO>() as u32,
        );
    }

    0
}

fn main() {
    println!("Hello, world!");
}
