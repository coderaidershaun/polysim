<!-- rex:codebase -->

## Codebase map

Run one of these commands before you search the tree by hand. Each one prints a tree of the working directory and skips the files that git ignores.

- `rex codebase`: every file.
- `rex codebase --rust-only`: only `.rs` files.
- `rex codebase --with-context`: adds the first sentence of each Rust module's `//!` doc comment to its line.
- `rex codebase --rust-only --with-context`: the Rust modules and what each one is for. Start here in a Rust crate.

Add `--for-human` to write the tree to `CODEBASE.md` instead of printing it, for example `rex codebase --for-human --rust-only --with-context`.

<!-- instructions -->

Read @CONSTITUTION.md

When managing sub-agebts, make sure their context windows do not exceed 70%. If they do, refresh with a new subagent.

Working on src/adapters/? Read src/adapters/exec/README.md first — it is the adapter execution contract.
