# bare-sidecar-rust

Start and manage Bare sidecar processes from Rust, each running Bare alongside the application with an IPC to it exposed as `Bare.IPC` inside the process.

```
cargo add bare-sidecar
```

## Usage

```rust
use std::io::{Read, Write};

fn main() -> std::io::Result<()> {
    let mut sidecar = bare_sidecar::spawn("app.js")?;

    sidecar.write_all(b"Hello world")?;

    let mut data = [0; 11];
    sidecar.read_exact(&mut data)?;

    println!("{}", String::from_utf8_lossy(&data));

    Ok(())
}
```

Where `app.js` is a Bare entry point using `Bare.IPC`:

```js
Bare.IPC.on('data', (data) => Bare.IPC.write(data))
```

Reading from the sidecar ends once the process has ended `Bare.IPC` and exited, so its exit status is known by then. `Sidecar::close()` closes the connection, terminates the process, and waits for it to exit. Dropping the sidecar does the same, but waits for the process in the background.

## Versions

`bare_sidecar::versions()` returns the versions of the components bundled with the crate, named as in `Bare.versions`:

```rust
for (name, version) in bare_sidecar::versions() {
    println!("{name} {version}");
}
```

The versions are read from `versions.json` at the root of the repository, which is the single source of truth for the version of Bare that the prebuilds are built with.

## License

Apache-2.0
