use postgres::Client;

pub fn load(client: &mut Client) {
    client.query("SELECT id FROM accounts", &[]);
}
