When adding new features, always write tests first and watch the tests fail. This is important. Then fix the code so the tests pass. Unless it's impossible to test, of course.

Also, every added feature should be scriptable.

Use the app's scriptability to test issues. As you do this and find deficiencies in the scripting feature's abilities, file issues to fix those problems (and fix them if you can).

Run `cargo test` to make sure tests pass. Fix warnings from `cargo build` as they come up. Also sometimes run `cargo run --exit` to make sure it launches.

We're in pre-alpha right now, so don't worry about defining schema migrations at this point -- just alter the initial schema. And don't mind backward compatibility. Feel free to make breaking compatibility changes.

For every completed task/feature/fix, record the change with `changer add ...` (see `changer add --help` for more info)