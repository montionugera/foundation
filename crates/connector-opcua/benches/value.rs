//! The ns of the decode of each value of a read of 4096 values: of Double, Int16, or
//! Boolean alone, or of the 11 built-in types in turn. Run with `cargo bench -p
//! connector-opcua --bench value`.

use connector_opcua::bench::value::Values;
use divan::Bencher;
use divan::counter::ItemsCount;

const COUNT: usize = 4096;

fn main() {
    divan::main();
}

fn read(bencher: Bencher<'_, '_>, ids: &[u8]) {
    let values = Values::new(ids, COUNT);
    assert_eq!(values.read(), COUNT, "each value decodes");
    bencher
        .counter(ItemsCount::new(COUNT))
        .bench_local(|| divan::black_box(&values).read());
}

#[divan::bench]
fn f64(bencher: Bencher<'_, '_>) {
    read(bencher, &[11]);
}

#[divan::bench]
fn i16(bencher: Bencher<'_, '_>) {
    read(bencher, &[4]);
}

#[divan::bench]
fn bool(bencher: Bencher<'_, '_>) {
    read(bencher, &[1]);
}

#[divan::bench]
fn mixed(bencher: Bencher<'_, '_>) {
    read(bencher, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]);
}
