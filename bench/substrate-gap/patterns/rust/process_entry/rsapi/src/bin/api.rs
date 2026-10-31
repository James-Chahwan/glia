use std::net::TcpListener;

fn main() {
    let listener = TcpListener::bind("0.0.0.0:8080").expect("bind");
    for stream in listener.incoming().flatten() {
        drop(stream);
    }
}
