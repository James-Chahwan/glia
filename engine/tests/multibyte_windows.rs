//! LA.25a gate: no cross-cutting extractor window cuts a UTF-8 char.
//!
//! The extractors slice fixed-width byte windows around a needle
//! (`&source[pos..pos + 256]`, `&source[pos - 256..pos]`). When a multibyte
//! char sits on the cut the slice panics, per-file isolation swallows the
//! panic, and the WHOLE file vanishes from the graph — glia's own
//! `parsers/code/extractors/src/config.rs` did, via a Cypher window opened by a
//! Rust `match`. This test drives every needle through `generate_one`, so it
//! runs exactly what `apply_cross_cutting_extractors` runs, whatever that
//! becomes, instead of coupling to each extractor's signature.
//!
//! LA.22c adds a `.clj` arm: every needle is also written as Clojure, plus
//! [`CLJ_NEEDLES`] for the Clojure parser's own text scan, whose reitit
//! look-back sliced `&source[i - 32..i]` before every `"`.
//!
//! LA.25b adds a `.rs` arm: every needle is also written as Rust, plus
//! [`RS_NEEDLES`] for the Rust parser's Tide / Poem / Salvo path-anchor scan,
//! whose verb window sliced `&source[..after + 256]` past the anchor's `)`.

use std::path::Path;

use repo_graph_code_domain::node_kind;
use repo_graph_engine::generate_one;

/// One anchor per cross-cutting extractor needle (54). Each is written next to
/// a 4-byte char at every pad that puts a window cut inside it.
const NEEDLES: &[&str] = &[
    "MATCH (p:Person) ",
    "merge ",
    "create ",
    "match x ",
    "class Settings:",
    "cronTime: '* * * * *', ",
    "'schedule': crontab(minute=0),",
    "\"schedule\": 30.0,",
    "cron.schedule('* * * * *', ",
    "@Scheduled(cron = \"0 0 * * *\")",
    "process.env.",
    "os.Getenv(\"X\")",
    "useQuery(GET_USERS)",
    "type Query {",
    "publish('x', ",
    ".emit('x', ",
    "@EventListener",
    "new WebSocket('ws://h/ws')",
    "producer.send({ topic: 'orders' ",
    "consumer.subscribe({ topic: 'orders' })",
    "channel.basic_publish(",
    "@KafkaListener(topics = \"orders\")",
    "app.get('/users', ",
    "router.post('/users', ",
    "fetch('/api/users')",
    "axios.get('/api/users')",
    "gql`query GetUsers { u }`",
    "@Query(",
    "program.command('build')",
    "subprocess.run(['glia', ",
    "db.collection('users')",
    "SELECT * FROM users WHERE ",
    "mongoose.model('User', ",
    "__tablename__ = 'users'",
    "TableName: 'orders'",
    "grpc.Dial(",
    "NewUserServiceClient(",
    "@Component({ selector: 'app-x' ",
    "createTRPCProxyClient(",
    "t.procedure.query(",
    "io.on('connection', ",
    "socket.emit('x', ",
    "System.getenv(\"X\")",
    "ENV['X']",
    "redis.publish('chan', ",
    "nc.Publish(\"orders\", ",
    "sqs.sendMessage({ QueueUrl: ",
    "@SqsListener(\"q\")",
    "export const handler = ",
    "defineStore('x', ",
    "useEffect(() => ",
    "http.HandleFunc(\"/x\", ",
    "@app.route('/x')",
    "@router.get('/x')",
];

/// The Clojure parser's own route / client needles (LA.22c), written as
/// `.clj` only: a reitit route vector, a compojure route and a clj-http call.
const CLJ_NEEDLES: &[&str] = &[
    "[\"/users\" {:get list-users}]",
    "(GET \"/users\" [] h)",
    "(client/get \"http://api/users\" {:headers {}})",
];

/// The Rust parser's path-anchor needles (LA.25b), written as `.rs` only. Each
/// ends on the anchor call's `)`, where the verb window opens, so the
/// needle-end pads of [`cut_pads`] land a char on the window's cut.
const RS_NEEDLES: &[&str] = &[
    "app.at(\"/health\")",
    "Route::new().at(\"/api/users\", get(list_users))",
    "Router::with_path(\"/users\")",
];

const WIDTHS: [usize; 5] = [32, 64, 128, 256, 512];
const EXTS: [&str; 4] = ["ts", "py", "clj", "rs"];
const WIDE: char = '\u{1F600}';

/// Pads `k` for which a `W`-wide window anchored at the needle's start or end
/// (forward) — or a `W`-byte look-back from its start or end — cuts inside the
/// 4-byte char: `(W-3)..=(W-1)` and `(W-len-3)..=(W-len-1)`, negatives dropped.
fn cut_pads(needle_len: usize) -> Vec<usize> {
    let mut ks: Vec<usize> = WIDTHS
        .iter()
        .flat_map(|&w| {
            let w = w as i64;
            let len = needle_len as i64;
            ((w - 3)..w).chain((w - len - 3)..(w - len))
        })
        .filter_map(|k| usize::try_from(k).ok())
        .collect();
    ks.sort_unstable();
    ks.dedup();
    ks
}

/// Writes the sweep tree under `root`; returns the file count.
fn write_sweep(root: &Path) -> usize {
    let arms: [(&str, &[&str], &[&str]); 3] = [
        ("n", NEEDLES, &EXTS),
        ("c", CLJ_NEEDLES, &["clj"]),
        ("r", RS_NEEDLES, &["rs"]),
    ];
    let mut files = 0;
    for (tag, needles, exts) in arms {
        files += write_arm(root, tag, needles, exts);
    }
    files
}

/// One arm of the sweep: every needle in `needles`, as every ext in `exts`.
fn write_arm(root: &Path, tag: &str, needles: &[&str], exts: &[&str]) -> usize {
    let mut files = 0;
    for (i, needle) in needles.iter().enumerate() {
        for ext in exts {
            let dir = root.join(format!("{tag}{i}_{ext}"));
            std::fs::create_dir_all(&dir).unwrap();
            for k in cut_pads(needle.len()) {
                let pad = "x".repeat(k);
                std::fs::write(
                    dir.join(format!("f{k}.{ext}")),
                    format!("{needle}{pad}{WIDE} y\n"),
                )
                .unwrap();
                std::fs::write(
                    dir.join(format!("b{k}.{ext}")),
                    format!("{WIDE}{pad}{needle} y\n"),
                )
                .unwrap();
                files += 2;
            }
        }
    }
    files
}

#[test]
fn extractor_windows_never_cut_a_multibyte_char() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let files = write_sweep(&repo);

    let r = generate_one(repo.to_str().unwrap()).unwrap();
    eprintln!(
        "[multibyte-sweep] files={files} needles={} widths=32,64,128,256,512 parse_errors={} clj_needles={} exts={} rs_needles={}",
        NEEDLES.len(),
        r.parse_errors.len(),
        CLJ_NEEDLES.len(),
        EXTS.join(","),
        RS_NEEDLES.len(),
    );
    assert!(
        r.parse_errors.is_empty(),
        "{} file(s) failed to parse; first 20:\n{}",
        r.parse_errors.len(),
        r.parse_errors
            .iter()
            .take(20)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// The glia self-build regression, reduced: a Rust `match` opens a 256-byte
/// Cypher window whose end lands inside `≤`. Before LA.25a the file produced
/// one parse error and zero nodes — not even its MODULE.
#[test]
fn window_cut_in_a_rust_file_keeps_the_file() {
    let head = "fn pick(t: &str) -> &str {\n    match t.find(\" #\") {\n        Some(i) => &t[..i],\n        None => t,\n    }\n}\n/// ";
    let m = head.find("match").unwrap();
    let pad = "x".repeat(m + 255 - head.len());
    let src = format!(
        "{head}{pad}\u{2264} 128\nfn is_valid_env_name(s: &str) -> bool {{ !s.is_empty() }}\n"
    );
    assert_eq!(src.find('\u{2264}'), Some(m + 255));
    assert!(!src.is_char_boundary(m + 256));

    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(repo.join("src/config.rs"), src).unwrap();

    let r = generate_one(repo.to_str().unwrap()).unwrap();
    assert!(r.parse_errors.is_empty(), "{:?}", r.parse_errors);
    let found = r.merged.graphs.iter().any(|g| {
        g.nav.qname_by_id.iter().any(|(id, q)| {
            q == "src::config::is_valid_env_name"
                && g.nav.kind_by_id.get(id) == Some(&node_kind::FUNCTION)
        })
    });
    assert!(
        found,
        "FUNCTION src::config::is_valid_env_name missing from the graph"
    );
}
