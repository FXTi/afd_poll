use miow::Overlapped;
use ntapi::ntioapi::NtDeviceIoControlFile;
use std::mem::size_of;
use winapi::km::basedef::IO_STATUS_BLOCK;
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

fn afd_poll(afd_helper_handle: HANDLE, poll_info: &AFD_POLL_INFO, overlapped: &Overlapped) -> i32 {
    let mut iosb = IO_STATUS_BLOCK::new();
    iosb.Status = winapi::ntstatus::STATUS_PENDING;
    unsafe {
        NtDeviceIoControlFile(
            afd_helper_handle,
            0 as *mut _,
            None,
            overlapped.raw(),
            &iosb as *mut _,
            0x00012024,
            &poll_info as *mut _,
            size_of::<AFD_POLL_INFO>(),
            &poll_info as *mut _,
            size_of::<AFD_POLL_INFO>(),
        );
    }

    0
}

fn main() {
    println!("Hello, world!");
}
