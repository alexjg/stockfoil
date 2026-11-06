//! Core `DecodeItem` and `PayloadCheck` state machines.
mod support;
use stockfoil::*;
use support::*;

fn fragment_source() -> (TestDocument, Vec<CommitHash>) {
    let alice = key(1);
    let mut g = document();
    let mut hashes = vec![signed(&mut g, &alice, vec![], b"root")];
    for i in 0..3 {
        let tip = *hashes.last().unwrap();
        hashes.push(signed(
            &mut g,
            &alice,
            vec![tip],
            format!("member {i}").as_bytes(),
        ));
    }
    let tip = *hashes.last().unwrap();
    let payload = payload_at_depth(&alice, &[tip], 1, "head");
    hashes.push(signed(&mut g, &alice, vec![tip], &payload));
    (g, hashes)
}

#[test]
fn decode_item_state_machine_streams_and_stops_at_last_wanted_member() {
    let (a, hashes) = fragment_source();
    let plan = a.export(&ExportCursor::new());
    let item = plan.item(0).map(|(_, bytes)| bytes).unwrap();

    let mut options = ReceiveOptions::new(format());
    options.read_size = 1;
    options.decode_capacity = 1;
    let mut op = a.history().decode_item(options, [(1, hashes[1])]).unwrap();

    let DecodeNext::Request(ticket, Request::Read { offset, max }) = op.next() else {
        panic!("expected initial read request")
    };
    assert!(matches!(op.next(), DecodeNext::AwaitReply));
    assert_eq!(
        op.reply(
            Ticket::from_raw(ticket.get() + 1),
            Reply::Read {
                bytes: &item[..1],
                eof: false
            }
        ),
        Err(ReplyError::Stale)
    );
    assert_eq!(
        op.reply(ticket, Reply::Bound(&[0])),
        Err(ReplyError::WrongKind)
    );
    op.reply(
        ticket,
        Reply::Read {
            bytes: &item[offset as usize..(offset as usize + max).min(item.len())],
            eof: offset as usize + max >= item.len(),
        },
    )
    .unwrap();

    let mut decoder: Option<ChunkDecoder> = None;
    let mut collected = Vec::new();
    loop {
        match op.next() {
            DecodeNext::Chunk { member, bytes } => {
                assert_eq!(member, 1);
                collected.extend_from_slice(bytes);
            }
            DecodeNext::Done => break,
            DecodeNext::Request(ticket, Request::Read { offset, max }) => {
                let start = (offset as usize).min(item.len());
                let end = (start + max).min(item.len());
                op.reply(
                    ticket,
                    Reply::Read {
                        bytes: &item[start..end],
                        eof: end == item.len(),
                    },
                )
                .unwrap();
            }
            DecodeNext::Request(
                ticket,
                Request::Decode {
                    meta,
                    input,
                    eof,
                    capacity,
                },
            ) => {
                let d = decoder.get_or_insert_with(|| ChunkDecoder::new(meta.len()));
                let mut out = vec![0u8; capacity];
                let p = d.step(input, eof, &mut out).unwrap();
                op.reply(
                    ticket,
                    Reply::Decoded {
                        consumed: p.consumed,
                        output: &out[..p.produced],
                        status: p.status,
                    },
                )
                .unwrap();
            }
            other => panic!("unexpected state: {other:?}"),
        }
    }
    assert_eq!(collected, b"member 0");
    assert!(
        (op.counts().bytes_read as usize) < item.len(),
        "read {} of {} bytes",
        op.counts().bytes_read,
        item.len()
    );
}

#[test]
fn decode_item_rejects_corrupt_or_mismatched_members() {
    let (a, hashes) = fragment_source();
    let plan = a.export(&ExportCursor::new());
    let item = plan.item(0).map(|(_, bytes)| bytes).unwrap();

    let run_decode = |wanted: (usize, CommitHash)| -> Result<(), Error> {
        let mut op = a
            .history()
            .decode_item(ReceiveOptions::new(format()), [wanted])?;
        let mut decoder: Option<ChunkDecoder> = None;
        loop {
            match op.next() {
                DecodeNext::Chunk { .. } => {}
                DecodeNext::Done => return Ok(()),
                DecodeNext::Failed(e) => return Err(e.clone()),
                DecodeNext::Request(ticket, Request::Read { offset, max }) => {
                    let start = (offset as usize).min(item.len());
                    let end = (start + max).min(item.len());
                    op.reply(
                        ticket,
                        Reply::Read {
                            bytes: &item[start..end],
                            eof: end == item.len(),
                        },
                    )
                    .unwrap();
                }
                DecodeNext::Request(
                    ticket,
                    Request::Decode {
                        meta,
                        input,
                        eof,
                        capacity,
                    },
                ) => {
                    let d = decoder.get_or_insert_with(|| ChunkDecoder::new(meta.len()));
                    let mut out = vec![0u8; capacity];
                    let p = d.step(input, eof, &mut out).unwrap();
                    op.reply(
                        ticket,
                        Reply::Decoded {
                            consumed: p.consumed,
                            output: &out[..p.produced],
                            status: p.status,
                        },
                    )
                    .unwrap();
                }
                _ => unreachable!(),
            }
        }
    };

    // Member 0 is not the head's payload.
    assert_eq!(
        run_decode((0, hashes[4])),
        Err(Error::CorruptPayload(hashes[4]))
    );
    // The item has no member 99.
    assert_eq!(
        run_decode((99, hashes[0])),
        Err(Error::CorruptPayload(hashes[0]))
    );
    // Undelivered commit.
    let unknown = CommitHash::from([7; 32]);
    assert_eq!(run_decode((0, unknown)), Err(Error::UnknownCommit(unknown)));
}
