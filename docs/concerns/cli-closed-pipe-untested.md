# The CLI's handling of a closed pipe has no test

`ctrlassist` writes its output with `writeln!` and ends quietly when the reader has gone, as `ctrlassist pads | head -1` makes happen (`crates/ctrlassist-cli/src/main.rs`). The handling was copied from TisVlay, which holds it with a test that closes the pipe on a command mid-output; that test wasn't brought over.

**Risk.** Low. A change that reintroduces a panic or an error message on a closed pipe would pass every gate. The lint against `println!` and `eprintln!` covers the commonest way in.

**Mitigation.** None.

**Closes when.** The CLI's tests run a command with enough output to fill a pipe, close the reading end, and see it exit as TisVlay's `workflows.rs` does.
