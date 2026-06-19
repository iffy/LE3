When adding new features, always write tests first to assert that the feature is implemented correctly (unless it's impossible to test).

Also, every added feature should be scriptable.

Run `cargo test` to make sure tests pass. Fix warnings from `cargo build` as they come up. Also sometimes run `cargo run --exit` to make sure it launches.

We're in pre-alpha right now, so don't worry about defining schema migrations at this point -- just alter the initial schema.
