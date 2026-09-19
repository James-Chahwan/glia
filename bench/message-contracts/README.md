# bench/message-contracts — message-contract (A12) fixture + smoke test

An offline check that **`glia contracts`** gives the right answer end to end.
The substrate-gap fixtures (`xcut-queue-msgtype-go`,
`xcut-queue-msgtype-go-mismatch`) show that each queue node has a
`MESSAGE_TYPE` cell. This bench checks the **report** built from those cells:
that the pairing, the verdict and the confidence are right, and that the CLI and
the pyo3 surface return the same rows.

## Pieces

- `svc/` — the publishing service (Go, NATS):
  - `publisher.go` marshals `&pb.OrderCreated{}` and runs `nc.Publish("orders", data)`.
  - `shipping.go` marshals `&pb.ShipmentCreated{}` and runs `nc.Publish("shipments", data)`.
- `worker/` — the consuming service:
  - `consumer.go` runs `nc.Subscribe("orders", …)` and unmarshals `&pb.OrderCreated{}`.
    Both sides use the same type, so this pair is the **match** row.
  - `shipping_worker.go` runs `nc.Subscribe("shipments", …)` and unmarshals
    `&pb.ShipmentDispatched{}`. The publisher sent a `ShipmentCreated`, so this
    pair is the **mismatch** row.

The two directories are merged as two repos (`--with`). Each producer/consumer
pair therefore crosses a repo boundary, as it does in a real two-service stack.

**Why four files.** The packet spec said a file could carry only one topic,
because `extract_topic_near` read only the first occurrence of each needle.
That is no longer true. `emit_queue_nodes`
(`parsers/code/extractors/src/queues.rs`) now scans every occurrence. It gives
each topic its own node, and `message_type_at` takes the type nearest each call
site. The fixture still uses one topic per file so that every row points to a
single file, which the `where` assertion in `check.sh` relies on.

**Why Go only.** The Go struct-literal scanner is the most direct way to get a
literal topic on both sides plus a schema type (not a primitive) on both
sides. A clean pair then gives `strong`, the only confidence that separates a
real mismatch from noise. Java and C# Kafka also produce literal topics now
(A2.2), and their generic payload types are probed in
`bench/substrate-gap/fixtures/java-spring-kafka-msgtype` and
`csharp-kafka-msgtype`. A Spring `KafkaTemplate<String, String>` producer reads
`String`, a primitive. Against a schema type that row is `unknown`, and
`String` vs `String` is only a **weak** `match`. That is why neither language
is used for the strong rows here.

## Run

```sh
bash bench/message-contracts/check.sh

# or by hand:
cargo run -p glia-cli -- contracts bench/message-contracts/svc \
    --with bench/message-contracts/worker            # markdown table
cargo run -p glia-cli -- contracts bench/message-contracts/svc \
    --with bench/message-contracts/worker --json     # the rows check.sh asserts
```

The pyo3 half imports the **installed** `repo_graph_py` wheel, not the working
tree. After a Rust change, rebuild the wheel before trusting that half. Run
`cargo clean -p glia-engine -p glia-py` before `maturin build`,
otherwise maturin can repackage a stale `.so`. The script **fails** if the
wheel is missing or has no `contracts()`. It never skips that half.

## Expected (asserted by `./check.sh`)

- The engine's stderr marker, matched exactly:
  `[contracts] topics=2 match=1 mismatch=1 unknown=0 literal=2 tag=0 rows=2`
- **1 match**: `orders`, `OrderCreated` → `OrderCreated`, confidence `strong`,
  `publisher.go` → `consumer.go`
- **1 mismatch**: `shipments`, `ShipmentCreated` → `ShipmentDispatched`,
  confidence `strong`, `shipping.go` → `shipping_worker.go`
- **0 unknown**: no unpaired row and no framework-tag row. A topic with no
  counterpart would add its own one-sided `unknown` row, so this count also
  catches phantom topics.
- No row has `topic_is_tag` or `pattern` set, and in every row the producer and
  consumer have different `repo_id`s.
- `PyGraph.contracts()` from `repo_graph_py.generate_many([svc, worker])`
  returns the same (topic, status, confidence, tag, producer type, consumer
  type) rows as the CLI, and prints `[contracts] surface=pyo3 repos=2`.

On success it prints:

```
[contracts] topics=2 match=1 mismatch=1 unknown=0 literal=2 tag=0 rows=2
PASS: 1 match (orders/OrderCreated), 1 mismatch (shipments/ShipmentCreated vs ShipmentDispatched)
PASS: pyo3 contracts() agrees with the CLI
```

## What is NOT covered here

`check.sh` runs `cargo run` and the installed wheel, so it is **not** part of
`cargo test --workspace`. Nothing runs it automatically, so it will go stale
unless someone runs it. Run it together with `bench/doc-link/check.sh` before a
release. The engine-level unit coverage is
`engine/tests/message_contracts.rs`, which includes the tag cross-product guard
`tag_topics_never_cross_product`.
