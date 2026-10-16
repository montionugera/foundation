//! The time to decode each header of a record's table, as the scan of a read and the
//! walk of an open do: a table of 1023 headers with samples, and one in which the first
//! header and every second one after it have no samples.

use buffer::bench::Table;
use divan::Bencher;
use divan::counter::ItemsCount;

const HEADERS: u32 = 1023;

fn main() {
    divan::main();
}

#[divan::bench(args = [0, 2])]
fn decode(bencher: Bencher<'_, '_>, empty: u32) {
    let table = Table::new(HEADERS, empty);
    bencher
        .counter(ItemsCount::new(
            usize::try_from(HEADERS).expect("u32 fits usize"),
        ))
        .bench_local(|| divan::black_box(&table).decode());
}
