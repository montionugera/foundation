//! A warm `Reader::next` makes no heap allocation when its frame is in the transport:
//! it takes a block of the pool and the series bytes, and gives the frame. This binary
//! has no test harness: the count covers each thread, and a harness allocates on its
//! own thread at any time.

#![expect(clippy::disallowed_macros, reason = "COUNTING ALLOCATOR")]

#[path = "../../../hub/tests/common/agent.rs"]
#[expect(dead_code, reason = "the binary uses only some of the helpers")]
mod agent;
#[path = "../common/connect.rs"]
mod connect;
#[path = "../common/hand.rs"]
mod hand;
#[path = "../common/home.rs"]
#[expect(dead_code, reason = "the binary uses only some of the helpers")]
mod home;
#[path = "../../../hub/tests/common/net.rs"]
#[expect(dead_code, reason = "the binary uses only some of the helpers")]
mod net;
#[path = "../../../hub/tests/common/node.rs"]
mod node;
#[path = "../../../hub/tests/common/sessions.rs"]
#[expect(dead_code, reason = "the binary uses only some of the helpers")]
mod sessions;

use std::pin::pin;
use std::task::{Context, Poll, Waker};

use types::channel;
use types::frame::Range;
use types::name::Selector;
use types::sample::{Scalar, Type};
use types::time::Span;
use wire::hub::client::{Mode, Place};

use crate::hand::{by_hand, opened, send_frame};

#[global_allocator]
static ALLOCATOR: counting::Allocator = counting::Allocator::new();

/// Frames that warm the reader and the pool up.
const WARM: u64 = 4;
/// Frames whose `next` is counted.
const COUNTED: u64 = 100;
/// The time after the open in which the node sends each frame.
const SENT: Span = Span::SECOND;

fn main() {
    takes_a_frame_with_no_allocation();
}

/// Each warm call of `next` on a frame that the node sent gives the frame at the first
/// poll, and it and the drop of the frame make no allocation.
fn takes_a_frame_with_no_allocation() {
    let place = |key, data_type, channel: &str| Place {
        key: channel::Key::from_u128(key),
        data_type,
        name: channel.parse().expect("a valid name"),
    };
    let places = [
        place(10, Type::Scalar(Scalar::I64), "a"),
        place(1, Type::Scalar(Scalar::Stamp), "time"),
        place(11, Type::Scalar(Scalar::I64), "b"),
    ];
    let body: Vec<u8> = (0..24).collect();
    by_hand(
        430,
        move |session, node| async move {
            let mut read = opened(&session, &places, 1).await;
            for seq in 0..WARM + COUNTED {
                let range = Range { seq, count: 1 };
                let run = [(0, 8), (1, 16), (2, 24)];
                send_frame(&mut read, range, &run, &body, &[24]).await;
            }
            node.clock().sleep(SENT).await;
            node.clock().sleep(SENT).await;
        },
        move |session, node| {
            Box::pin(async move {
                let select = Selector::new(["a", "b"]).expect("a selector");
                let mut reader =
                    session.reader(select, Mode::Complete).await.expect("opens");
                node.clock().sleep(SENT).await;
                let mut context = Context::from_waker(Waker::noop());
                for seq in 0..WARM + COUNTED {
                    let (taken, count) = ALLOCATOR.count(|| {
                        let Poll::Ready(frame) = pin!(reader.next()).poll(&mut context)
                        else {
                            return false;
                        };
                        let frame = frame.expect("a frame");
                        frame.range(0) == Some(Range { seq, count: 1 })
                    });
                    assert!(taken, "frame {seq} is ready and in order");
                    if seq >= WARM {
                        assert_eq!(count, 0, "frame {seq}");
                    }
                }
            })
        },
    );
}
