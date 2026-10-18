- **SDK WRITER** `sdk::Client::open_writer(write::Config)` opens a `sdk::write::Writer`
  at the client's node, which must be the home of the index of its channels (BQ12, until
  #3104). `write::Config::new(channels)` gives authority 0 and no lease, so a writer of
  a higher authority, such as a connector, takes control from it. The client's thread
  opens a `program::Writer` in a task of its own, which serves the calls of its writer
  until the writer closes or drops, or the client ends. Each call copies its samples
  into a frame that the writer keeps, sends it to the task on a channel of 1, and
  blocks on a `std` channel of 1 for the outcome and the frame. So a warm write takes no
  allocation, and no draft is held across calls. `Writer` is `Send` and holds no borrow
  of `Client`. The plan
  (https://github.com/synnaxlabs/foundation/issues/3266#issuecomment-6255057555) was
  approved by `laptop.architect-2` at 2026-10-18T21:04:06Z
  (https://github.com/synnaxlabs/foundation/issues/3266#issuecomment-6255123001), with
  these terms:
  1. `Error::Stream(program::Error)`: the node refused the open of a stream, or a call
     of a writer failed, and the session goes on. A call gives `Error::Session` with the
     cause only when the session ended. `Error::Closed` comes only after
     `Client::close` or a drop of the client.
  2. A `Value::Stamp` at the index place is the stamp of the frame, sent with
     `Time::Frame(Path::Live)`. With no value there, the frame goes with `Time::Node`.
  3. `write` and `write_series` take the places in any order. `sdk` sorts them in the
     kept frame. A place given twice gives `Stream(Layout)`.
  4. A place past the places gives `Stream(Layout(OutOfRange))`, so `Error::Value {
     place, scalar, data_type }` means only a value whose type is not the type of its
     place.
  5. `Writer::places` gives the places in key order, and the index is a place also when
     `Config::channels` does not name it.
  6. One request channel into the client's thread, which `Client` holds next to the
     `Mutex` of its thread. Each send is a `try_send` or an unbounded send, so no send
     blocks or panics in an async context.
  A series whose bytes are not `count` samples of the type of its place gives
  `Ok(Outcome::Refused(Refusal::Codec))`, and the writer goes on. Lost: a borrowed
  `Writer<'_>` of `Client`, because a PyO3 class holds no borrow, so a Python close
  could not end a session with a live writer.
