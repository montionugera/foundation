use proptest::prelude::*;

use super::*;

fn head(time: Time, form: Form, count: u32) -> Head {
    Head {
        time,
        form,
        count,
        series: 2,
    }
}

fn encode_head(head: Head) -> Vec<u8> {
    let mut out = vec![0xaa; Head::LEN];
    head.encode(&mut out);
    out
}

fn encode_reply(reply: Reply) -> Vec<u8> {
    let mut out = vec![0xaa; reply.encoded_len()];
    reply.encode(&mut out);
    out
}

/// `kind`, then `len - 1` zeros.
fn zeros(kind: u8, len: usize) -> Vec<u8> {
    let mut bytes = vec![0; len];
    bytes[0] = kind;
    bytes
}

fn taken(seq: u64, count: u32) -> Reply {
    Reply::Outcome(Outcome::Taken {
        range: frame::Range { seq, count },
    })
}

/// A reply of `kind` with the range `seq` and `count`.
fn range_bytes(kind: u8, seq: u64, count: u32) -> Vec<u8> {
    let mut bytes = vec![kind];
    bytes.extend(seq.to_le_bytes());
    bytes.extend(count.to_le_bytes());
    bytes
}

const REFUSALS: [(Refusal, u8); 7] = [
    (Refusal::Waiting, 1),
    (Refusal::Reserved, 2),
    (Refusal::Expired, 3),
    (Refusal::Order, 4),
    (Refusal::Codec, 5),
    (Refusal::Full, 6),
    (Refusal::Large, 7),
];

mod pins {
    use super::*;

    #[test]
    fn the_head() {
        let mut head = head(Time::Frame(Path::Live), Form::Raw, 0x0102);
        head.series = 0x0304;
        let mut bytes = vec![7, 0, 0, 0x02, 0x01, 0, 0, 0x04, 0x03, 0, 0];
        assert_eq!(encode_head(head), bytes);
        head.time = Time::Frame(Path::Backfill);
        head.form = Form::Encoded;
        bytes[1..3].copy_from_slice(&[1, 1]);
        assert_eq!(encode_head(head), bytes);
        let node = Head {
            time: Time::Node,
            form: Form::Raw,
            count: 1,
            series: 3,
        };
        assert_eq!(encode_head(node), [7, 2, 0, 1, 0, 0, 0, 3, 0, 0, 0]);
    }

    #[test]
    fn the_replies() {
        let range = frame::Range {
            seq: 0x0102,
            count: 0x0304,
        };
        let mut bytes = vec![1, 0x02, 0x01, 0, 0, 0, 0, 0, 0, 0x04, 0x03, 0, 0];
        assert_eq!(encode_reply(taken(0x0102, 0x0304)), bytes);
        bytes[0] = 2;
        let lost = Reply::Outcome(Outcome::Lost { range });
        assert_eq!(encode_reply(lost), bytes);
        for (refusal, code) in REFUSALS {
            let refused = Reply::Outcome(Outcome::Refused(refusal));
            assert_eq!(encode_reply(refused), [3, code]);
        }
        let stored = Reply::Stored { through: 0x0506 };
        assert_eq!(encode_reply(stored), [4, 0x06, 0x05, 0, 0, 0, 0, 0, 0]);
    }
}

mod head {
    use super::*;

    #[test]
    #[should_panic(expected = "a head has 0 samples of 2 series, not at least 1 of 1")]
    fn panics_on_an_encode_with_no_samples() {
        encode_head(head(Time::Frame(Path::Live), Form::Raw, 0));
    }

    #[test]
    #[should_panic(expected = "a head has 1 samples of 0 series, not at least 1 of 1")]
    fn panics_on_an_encode_with_no_series() {
        let head = head(Time::Frame(Path::Live), Form::Raw, 1);
        encode_head(Head { series: 0, ..head });
    }

    #[test]
    #[should_panic(
        expected = "a head that the node stamps has 1 samples in the Encoded form"
    )]
    fn panics_on_an_encode_of_an_encoded_node_head() {
        encode_head(head(Time::Node, Form::Encoded, 1));
    }

    #[test]
    #[should_panic(
        expected = "a head that the node stamps has 2 samples in the Raw form"
    )]
    fn panics_on_an_encode_of_a_node_head_of_two_samples() {
        encode_head(head(Time::Node, Form::Raw, 2));
    }
}

mod reply {
    use super::*;

    #[test]
    fn refuses_each_kind_that_no_reply_has() {
        for kind in (0..=u8::MAX).filter(|kind| !(1..=4).contains(kind)) {
            assert_eq!(Reply::decode(&zeros(kind, 13)), Err(Error::Kind { kind }));
        }
    }

    #[test]
    fn refuses_an_empty_message() {
        assert_eq!(Reply::decode(&[]), Err(Error::Empty));
    }

    #[test]
    fn refuses_each_wrong_length() {
        for (kind, len) in [(1, 12), (1, 14), (2, 1), (2, 14), (3, 1), (3, 3), (4, 8)] {
            assert_eq!(
                Reply::decode(&zeros(kind, len)),
                Err(Error::Length { len }),
                "{kind}"
            );
        }
        assert_eq!(Reply::decode(&zeros(4, 10)), Err(Error::Length { len: 10 }));
    }

    #[test]
    fn refuses_a_range_of_no_sample() {
        for kind in [1, 2] {
            assert_eq!(
                Reply::decode(&range_bytes(kind, 9, 0)),
                Err(Error::Count { seq: 9 })
            );
        }
    }

    #[test]
    fn refuses_a_range_that_ends_past_the_highest_seq() {
        for kind in [1, 2] {
            assert_eq!(
                Reply::decode(&range_bytes(kind, u64::MAX - 1, 2)),
                Err(Error::Range {
                    seq: u64::MAX - 1,
                    count: 2
                })
            );
        }
        assert_eq!(
            Reply::decode(&range_bytes(1, u64::MAX - 2, 2)),
            Ok(taken(u64::MAX - 2, 2))
        );
    }

    #[test]
    fn checks_the_length_before_the_range() {
        let mut long = range_bytes(1, u64::MAX, 0);
        long.push(0);
        assert_eq!(Reply::decode(&long), Err(Error::Length { len: 14 }));
    }

    #[test]
    fn refuses_a_code_that_no_refusal_has() {
        for code in (0..=u8::MAX).filter(|code| !(1..=7).contains(code)) {
            assert_eq!(Reply::decode(&[3, code]), Err(Error::Refusal { code }));
        }
    }

    #[test]
    fn checks_the_length_before_the_code() {
        assert_eq!(Reply::decode(&[3, 0, 0]), Err(Error::Length { len: 3 }));
    }

    #[test]
    fn decodes_each_refusal() {
        for (refusal, code) in REFUSALS {
            assert_eq!(
                Reply::decode(&[3, code]),
                Ok(Reply::Outcome(Outcome::Refused(refusal)))
            );
        }
    }

    #[test]
    fn outcome_len_max_is_the_len_of_the_longest_outcome() {
        let range = frame::Range { seq: 0, count: 1 };
        let mut outcomes = Vec::new();
        // A new outcome does not compile here until the list holds it.
        for outcome in [
            Outcome::Taken { range },
            Outcome::Lost { range },
            Outcome::Refused(Refusal::Waiting),
        ] {
            match outcome {
                Outcome::Taken { .. } | Outcome::Lost { .. } => outcomes.push(outcome),
                Outcome::Refused(_) => {
                    outcomes.extend(Refusal::ALL.map(Outcome::Refused));
                }
            }
        }
        let lens = outcomes
            .into_iter()
            .map(|outcome| Reply::Outcome(outcome).encoded_len());
        assert_eq!(lens.max(), Some(Outcome::LEN_MAX));
    }

    #[test]
    #[should_panic(
        expected = "a reply's range of 0 samples from seq 4 holds none or ends past \
                    the highest seq"
    )]
    fn panics_on_an_encode_of_a_range_of_no_sample() {
        encode_reply(taken(4, 0));
    }

    #[test]
    #[should_panic(
        expected = "a reply's range of 2 samples from seq 18446744073709551615 holds \
                    none or ends past the highest seq"
    )]
    fn panics_on_an_encode_of_a_range_past_the_highest_seq() {
        let range = frame::Range {
            seq: u64::MAX,
            count: 2,
        };
        encode_reply(Reply::Outcome(Outcome::Lost { range }));
    }
}

fn reply() -> impl Strategy<Value = Reply> {
    let range = (1..=u32::MAX).prop_flat_map(|count| {
        (0..=u64::MAX.saturating_sub(u64::from(count)))
            .prop_map(move |seq| frame::Range { seq, count })
    });
    prop_oneof![
        range
            .clone()
            .prop_map(|range| Reply::Outcome(Outcome::Taken { range })),
        range.prop_map(|range| Reply::Outcome(Outcome::Lost { range })),
        (0..REFUSALS.len())
            .prop_map(|at| Reply::Outcome(Outcome::Refused(REFUSALS[at].0))),
        any::<u64>().prop_map(|through| Reply::Stored { through }),
    ]
}

proptest! {
    #[test]
    fn round_trips_a_reply(reply in reply()) {
        prop_assert_eq!(Reply::decode(&encode_reply(reply)), Ok(reply));
    }
}
