use std::net;
use std::sync::Arc;

struct State {}

pub struct TcpStream {
    sock: net::TcpStream,
    state: Arc<State>,
}

impl TcpStream {
    fn new(socket: net::TcpStream) -> TcpStream {
        TcpStream {
            sock: socket,
            state: Arc::new(State {}),
        }
    }
}
