use winapi::shared::ntdef::{HANDLE, NULL};
use miow::Overlapped;

struct AFD_POLL_INFO {

}

fn afd_poll(afd_helper_handle: HANDLE, poll_info: &AFD_POLL_INFO, overlapped: &Overlapped) -> i32 {

}

fn main() {
    println!("Hello, world!");
}
