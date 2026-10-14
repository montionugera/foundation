use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::Relaxed;

use document::Document;
use document::diagnostic::{Code, Diagnostic};
use types::channel;
use types::name::Name;
use types::time::Span;

use super::{Rig, create_status};
use crate::cancel;
use crate::common::{env, run_on};
use crate::kind::{Channels, Context, Error, Kind, Table};
use crate::status::State;

const REFUSED: Code = Code::new("test.refused");

const TEN: Span = Span::from_nanos(10_000_000_000);

fn name(text: &str) -> Name {
    text.parse().expect("a valid name")
}

/// A kind that adds 1 to its count `ticks` each 100 ms until the cancel. Its run
/// spawns a task that sets `lingered` 2 s after the cancel.
#[derive(Default)]
struct Ticks {
    lingered: Arc<AtomicBool>,
}

impl Kind for Ticks {
    type Config = ();

    fn parse(&self, _: &Document) -> Result<(), Vec<Diagnostic>> {
        Ok(())
    }

    fn check(&self, (): &()) -> Result<Channels, Vec<Diagnostic>> {
        Ok(Channels {
            counts: vec![name("ticks")],
            ..Channels::default()
        })
    }

    fn discover(
        &self,
        _: &cancel::Token,
    ) -> impl Future<Output = Result<Vec<Document>, Error>> {
        std::future::ready(Ok(Vec::new()))
    }

    async fn run(&self, ctx: Context<()>) -> Result<(), Error> {
        let (clock, cancel) = (ctx.clock().clone(), ctx.cancel().clone());
        let lingered = Arc::clone(&self.lingered);
        ctx.tasks().spawn(async move {
            cancel.wait().await;
            clock.sleep(Span::from_nanos(2_000_000_000)).await;
            lingered.store(true, Relaxed);
        });
        let ticks = ctx.count("ticks");
        while ctx
            .cancel()
            .race(ctx.clock().sleep(Span::from_nanos(100_000_000)))
            .await
            .is_some()
        {
            ticks.add(1);
        }
        Ok(())
    }
}

/// A kind whose check refuses each config. Its run ends with `Ok` at once.
struct Refuses;

impl Kind for Refuses {
    type Config = ();

    fn parse(&self, _: &Document) -> Result<(), Vec<Diagnostic>> {
        Ok(())
    }

    fn check(&self, (): &()) -> Result<Channels, Vec<Diagnostic>> {
        Err(vec![refused()])
    }

    fn discover(
        &self,
        _: &cancel::Token,
    ) -> impl Future<Output = Result<Vec<Document>, Error>> {
        std::future::ready(Ok(Vec::new()))
    }

    fn run(&self, _: Context<()>) -> impl Future<Output = Result<(), Error>> {
        std::future::ready(Ok(()))
    }
}

fn refused() -> Diagnostic {
    Diagnostic::new(REFUSED, None, "refused".into(), "Fix it".into())
}

/// Opens a rig of `kinds` on a sim node, and runs `test` on it.
fn on_rig<F: Future<Output = ()> + 'static>(
    kinds: impl FnOnce() -> Table + Send + 'static,
    test: impl FnOnce(Rig) -> F + Send + 'static,
) {
    run_on(move |node, tasks| async move {
        let rig = Rig::open(env(&node, tasks), node.net(), kinds()).await;
        test(rig).await;
    });
}

/// Whether `reading` is Running with no error, no restart, and a count of ticks above
/// 0.
fn ticking(reading: &super::Reading) -> bool {
    reading.state == State::Running
        && reading.error.is_none()
        && reading.restarts == 0
        && reading
            .counts
            .get(&name("ticks"))
            .is_some_and(|&ticks| ticks > 0)
}

#[test]
fn a_connector_reaches_running_and_stops_with_ok() {
    on_rig(
        || Table::new().with("ticks", Ticks::default()),
        |mut rig| async move {
            let config = Document::default();
            let mut run = rig.supervise("ticks", name("plant.ticks"), config).await;
            run.until(TEN, ticking).await;
            assert_eq!(run.stop().await.map_err(|e| e.to_string()), Ok(()));
        },
    );
}

#[test]
fn stop_returns_once_each_task_of_the_run_ended() {
    let ticks = Ticks::default();
    let lingered = Arc::clone(&ticks.lingered);
    on_rig(
        || Table::new().with("ticks", ticks),
        |mut rig| async move {
            let config = Document::default();
            let mut run = rig.supervise("ticks", name("plant.ticks"), config).await;
            run.until(TEN, ticking).await;
            assert_eq!(run.stop().await.map_err(|e| e.to_string()), Ok(()));
            assert!(lingered.load(Relaxed), "the task of the run ended");
        },
    );
}

#[test]
fn a_refused_config_ends_the_run_with_its_diagnostics_and_defines_nothing() {
    on_rig(
        || Table::new().with("refuses", Refuses),
        |mut rig| async move {
            let config = Document::default();
            let run = rig
                .supervise("refuses", name("plant.refuses"), config)
                .await;
            let Err(Error::Config(diagnostics)) = run.stop().await else {
                panic!("a refused config ends with `Config`")
            };
            assert_eq!(diagnostics, [refused()]);
            assert!(rig.definitions.is_empty(), "{:?}", rig.definitions);
        },
    );
}

#[test]
fn define_keeps_what_an_earlier_call_defined() {
    on_rig(Table::new, |mut rig| async move {
        let (a, b) = (name("plant.a"), name("plant.b"));
        rig.define(create_status(&a, &[], channel::Key::from_u128(1)));
        rig.define(create_status(&b, &[], channel::Key::from_u128(10)));
        let state = name("plant.a.status.state");
        rig.writer(name("plant.test"), vec![state]).await;
    });
}

#[test]
fn two_connectors_of_one_rig_both_run() {
    on_rig(
        || Table::new().with("ticks", Ticks::default()),
        |mut rig| async move {
            let config = Document::default();
            let mut a = rig
                .supervise("ticks", name("plant.a"), config.clone())
                .await;
            let mut b = rig.supervise("ticks", name("plant.b"), config).await;
            a.until(TEN, ticking).await;
            b.until(TEN, ticking).await;
            assert_eq!(a.stop().await.map_err(|e| e.to_string()), Ok(()));
            assert_eq!(b.stop().await.map_err(|e| e.to_string()), Ok(()));
        },
    );
}

#[test]
#[should_panic(
    expected = "no status within 1s matches, after [Reading { state: Running, restarts: 0"
)]
fn until_panics_with_the_readings_when_none_matches_in_time() {
    on_rig(
        || Table::new().with("ticks", Ticks::default()),
        |mut rig| async move {
            let config = Document::default();
            let mut run = rig.supervise("ticks", name("plant.ticks"), config).await;
            run.until(Span::SECOND, |reading| reading.restarts > 0)
                .await;
        },
    );
}

/// A drop of a [`super::Run`] cancels its connector, so the shard can end.
#[test]
fn a_drop_of_a_run_cancels_the_connector() {
    on_rig(
        || Table::new().with("ticks", Ticks::default()),
        |mut rig| async move {
            let config = Document::default();
            let run = rig.supervise("ticks", name("plant.ticks"), config).await;
            let ended = run.ended.clone();
            drop(run);
            let late = ended.race(rig.clock.sleep(TEN)).await;
            assert_eq!(late, None, "the run ends within 10 s of the drop");
        },
    );
}
