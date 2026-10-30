use postgres::Client;

fn seed(c: &mut Client) {
    c.query("SELECT id FROM helper_rows", &[]);
}

#[test]
fn reads() {
    let mut c = Client::connect("host=localhost", postgres::NoTls).unwrap();
    seed(&mut c);
    c.query("SELECT id FROM test_rows", &[]);
}
