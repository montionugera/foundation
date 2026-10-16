use std::{fmt, io};

use env::thread::Handle;
use env::threads::Threads;

use crate::files::Queue;

/// The standard input of the process, read on an I/O thread so that a read blocks no
/// shard. Clones read the same input. The thread ends after the last clone drops and
/// each read that it took ends.
#[derive(Clone)]
pub struct Input(Queue);

impl fmt::Debug for Input {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Input").finish_non_exhaustive()
    }
}

impl Input {
    /// Starts I/O thread `name` and gives the input and the handle of the thread.
    pub(crate) fn start(
        threads: &Threads,
        name: &str,
    ) -> Result<(Self, Handle), env::thread::Error> {
        let (queue, thread) = Queue::start(threads, name)?;
        Ok((Self(queue), thread))
    }

    /// What `read` gives, called on the I/O thread with the lock of standard input,
    /// after each read that this input took before it. Bytes that a read buffers stay
    /// for the next read. Once the thread takes `read`, it runs also when the future
    /// drops.
    ///
    /// # Panics
    ///
    /// When a panic ended the I/O thread, as a panic of `read` does.
    pub async fn read<T: Send + 'static>(
        &self,
        read: impl FnOnce(&mut io::StdinLock<'static>) -> T + Send + 'static,
    ) -> T {
        self.0.run(move || read(&mut io::stdin().lock())).await
    }
}

#[cfg(test)]
mod tests {
    /// A read runs on the I/O thread and gives its value, and the thread ends after
    /// the input drops.
    #[test]
    fn a_read_runs_on_the_thread_and_the_thread_ends_after_the_drop() {
        let threads = crate::threads().unwrap();
        let (input, thread) = crate::input(&threads, "input").unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let read = input.read(|_| std::thread::current().name().map(str::to_owned));
        assert_eq!(runtime.block_on(read).as_deref(), Some("input"));
        drop(input);
        thread.join().unwrap();
    }
}
