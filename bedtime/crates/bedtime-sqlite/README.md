# bedtime-sqlite

Read-only SQLite inspection without Bevy or desktop dependencies. `Request`
selects schema, paged table data, or a SQL query; `Worker` runs requests in the
background and returns typed `Response` values. Each worker has one replaceable
pending request, so cancelling or replacing a query does not affect another
panel's work.

Connections open existing databases read-only. The backend rejects writes,
limits query rows and result bytes, and interrupts long-running statements.
The SQLite browser plugin and Bedside Table share this backend while owning
their own presentation and persistence.

```sh
cargo test --locked -p bedtime-sqlite
```
