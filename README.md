# Playloop

Playloop plays and loops Ogg Vorbis audio files that have loop metadata in them. It expects Vorbis comments `LOOPSTART` and `LOOPEND` to be set to sample positions.

This project is written in Rust and uses [Lewton](https://github.com/RustAudio/lewton) for Vorbis decoding and [SDL](https://www.libsdl.org/) for sound output. It was primarily developed for fun, to learn Rust and to brush up on my audio programming skills. The program decodes and seeks on-the-fly, and uses minimal allocations (at least in the code I wrote; can't speak for the dependencies).

The looping is perfectly accurate. This was tricky to get right with Lewton/Vorbis due to the lack of precision seeking. For example, playloop will seek to an earlier Vorbis "granule" then skip N samples forward to reach the correct sample to play. There are automated tests to prove this works correctly. The tests decode an entire file into memory and compare the on-the-fly looped output against it.

## Building

This project can be built using Rust's `cargo` build tool. You'll also need SDL2 development libraries.

### Ubuntu

```sh
apt install libsdl2-dev
cargo build --release
```

After building, the executable will live at `target/release/playloop`.

### Mac

```sh
brew install sdl2
cargo build --release
```

After building, the executable will live at `target/release/playloop`.

### Windows

On Windows, building takes a little more effort. As a convenience, you can find a build for Windows [in the releases area](https://github.com/jkarneges/playloop/releases).

To build from source, see the [rust-sdl2](https://github.com/Rust-SDL2/rust-sdl2) setup docs.

## Usage

```sh
playloop file.ogg
```

## Running tests

```sh
cargo test
```

Note: the tests can take a long time since they decode a bunch of Vorbis data.
