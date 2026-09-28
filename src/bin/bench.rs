//! Times the pool, the regions and the frames in nanoseconds and writes
//! the table to `bench/results.md`.
#![deny(clippy::pedantic, missing_docs)]

use eorm::frames::{Frames, Stack};
use eorm::pool::{Blocks, Pool};
use std::hint::black_box;
use std::process::Command;
use std::time::{Duration, Instant};

/// A piece of work that can be repeated and timed.
///
/// Each repetition does a fixed number of operations, known to whoever
/// builds the work, so the time a run returns divided by the operations
/// it did is the cost of one of them.
trait Work {
    /// Repeats the work `reps` times and returns how long the timed part
    /// of it took.
    fn run(&mut self, reps: usize) -> Duration;
}

/// A number found out by running the code, not written down.
trait Quantity {
    /// The number, measured now.
    fn amount(&mut self) -> usize;
}

/// A piece of the results file, as Markdown.
trait Text {
    /// The Markdown of this piece, computing it if needed.
    fn text(&mut self) -> String;
}

/// One frame entered, filled with `count` allocations of `words` words
/// each, and left, all of it timed.
///
/// With `count` zero this is an empty frame's `enter` and `leave`; with
/// allocations of a whole block each it is a frame that fills `count`
/// blocks; with small allocations over many blocks it is `alloc` with
/// its block refills, `enter` and `leave` spread over thousands of them.
/// Every pointer `alloc` returns goes through `black_box`, so none of
/// the allocations can be optimized away.
struct Frame {
    stack: Stack<Blocks>,
    words: usize,
    count: usize,
}

impl Work for Frame {
    fn run(&mut self, reps: usize) -> Duration {
        let start = Instant::now();
        for _ in 0..reps {
            self.stack.enter();
            for _ in 0..self.count {
                black_box(self.stack.alloc(1, self.words));
            }
            self.stack.leave();
        }
        start.elapsed()
    }
}

/// The time of one work minus the time of another, run the same number
/// of times.
///
/// Two works that differ only in some operations cost the same except
/// for those operations, so the difference is what they alone cost,
/// with everything the two share taken out.
struct Difference<A: Work, B: Work> {
    whole: A,
    part: B,
}

impl<A: Work, B: Work> Work for Difference<A, B> {
    fn run(&mut self, reps: usize) -> Duration {
        self.whole.run(reps).saturating_sub(self.part.run(reps))
    }
}

/// `leave` of frames that filled `blocks` blocks each, timed apart
/// from the `enter` and the filling before it.
///
/// One `leave` takes less than a tick of the clock on some machines,
/// and the processor may run it alongside the clock readings, so
/// `frames` frames are entered one inside another and filled first,
/// untimed, and then all of them are left between two readings of the
/// clock. Each `leave` closes the youngest frame, exactly as a lone one
/// would, and the blocks of all frames together are as many as the
/// largest round trip fills, so their headers are as warm as there.
/// A second pair of readings with nothing between them is taken right
/// after and subtracted, so the clock's own cost does not count.
struct Leave {
    stack: Stack<Blocks>,
    words: usize,
    blocks: usize,
    frames: usize,
}

impl Work for Leave {
    fn run(&mut self, reps: usize) -> Duration {
        let mut spent = Duration::ZERO;
        let mut idle = Duration::ZERO;
        for _ in 0..reps {
            for frame in 1..=self.frames {
                self.stack.enter();
                for _ in 0..self.blocks {
                    black_box(self.stack.alloc(frame, self.words));
                }
            }
            let start = Instant::now();
            for _ in 0..self.frames {
                self.stack.leave();
            }
            spent += start.elapsed();
            let start = Instant::now();
            idle += black_box(start).elapsed();
        }
        spent.saturating_sub(idle)
    }
}

/// The number of words one block holds after its header.
///
/// It is measured, not computed from the block size, since the header
/// is the region's own business: a fresh frame takes one word at a time
/// until a word no longer lands right after the one before it, which is
/// the moment the frame took a new block.
struct Capacity {
    stack: Stack<Blocks>,
}

impl Quantity for Capacity {
    fn amount(&mut self) -> usize {
        self.stack.enter();
        let depth = self.stack.depth();
        let first = self.stack.alloc(depth, 1).addr().get();
        let words = 1
            + (1..Blocks::SIZE / 8)
                .take_while(|index| self.stack.alloc(depth, 1).addr().get() == first + index * 8)
                .count();
        self.stack.leave();
        words
    }
}

/// One block popped from the pool and pushed straight back.
///
/// The `unsafe` push is sound: the block comes from `pop` of this very
/// pool the line before, is not in the pool, and nothing touches it
/// after, since `black_box` only reads the pointer.
struct Cycle {
    pool: Blocks,
}

impl Work for Cycle {
    fn run(&mut self, reps: usize) -> Duration {
        let start = Instant::now();
        for _ in 0..reps {
            let block = black_box(self.pool.pop());
            unsafe { self.pool.push(block) };
        }
        start.elapsed()
    }
}

/// A row of the table: one work, timed with a warm-up and fifteen
/// samples.
///
/// The warm-up doubles the repetitions until the timed part of one run
/// takes 10 ms, which also warms the caches, the pool and the branch
/// predictors; every sample then runs that many repetitions. The row
/// shows the median nanoseconds per operation, the fastest and the
/// slowest sample, so noise is visible, and the operations timed per
/// sample.
struct Timing<W: Work> {
    what: &'static str,
    size: String,
    ops: usize,
    work: W,
}

impl<W: Work> Text for Timing<W> {
    fn text(&mut self) -> String {
        let mut reps = 1;
        while self.work.run(reps) < Duration::from_millis(10) {
            reps *= 2;
        }
        let total =
            f64::from(u32::try_from(reps * self.ops).unwrap_or_else(|_| {
                panic!("{} operations do not fit in one sample", reps * self.ops)
            }));
        let mut sample: Vec<f64> = (0..15)
            .map(|_| self.work.run(reps).as_secs_f64() * 1e9 / total)
            .collect();
        sample.sort_by(f64::total_cmp);
        let line = format!(
            "| {} | {} | {:.2} | {:.2}–{:.2} | {} |",
            self.what,
            self.size,
            sample[7],
            sample[0],
            sample[14],
            reps * self.ops
        );
        println!("{line}");
        line
    }
}

/// The trimmed output of a command, a fact about the machine gathered
/// at run time.
struct Shell {
    program: &'static str,
    args: &'static [&'static str],
}

impl Text for Shell {
    fn text(&mut self) -> String {
        let output = Command::new(self.program)
            .args(self.args)
            .output()
            .unwrap_or_else(|error| panic!("cannot run {}: {error}", self.program));
        assert!(
            output.status.success(),
            "{} {:?} fails with {}",
            self.program,
            self.args,
            output.status
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }
}

/// Rows of the table, one after another.
struct Rows {
    rows: Vec<Box<dyn Text>>,
}

impl Text for Rows {
    fn text(&mut self) -> String {
        self.rows
            .iter_mut()
            .map(|row| row.text())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// The rows that time `alloc` of 2, 4 and 8 words in the youngest
/// frame: once with the block refills and the frame's `enter` and
/// `leave` spread over 100 blocks of allocations, once for the fast
/// path alone.
///
/// The fast path is the difference between a frame that fills one
/// block with allocations and a frame that makes only the first of
/// them, the one that takes the block; what is left is the allocations
/// that only bump.
struct Allocs {
    block: usize,
}

impl Text for Allocs {
    fn text(&mut self) -> String {
        let spread = [2, 4, 8].map(|words| -> Box<dyn Text> {
            Box::new(Timing {
                what: "`alloc`, refills and `enter`/`leave` spread over 100 blocks",
                size: format!("{words} words"),
                ops: 100 * (self.block / words),
                work: Frame {
                    stack: Stack::new(Blocks::default()),
                    words,
                    count: 100 * (self.block / words),
                },
            })
        });
        let fast = [2, 4, 8].map(|words| -> Box<dyn Text> {
            Box::new(Timing {
                what: "`alloc` inside one block, fast path only",
                size: format!("{words} words"),
                ops: self.block / words - 1,
                work: Difference {
                    whole: Frame {
                        stack: Stack::new(Blocks::default()),
                        words,
                        count: self.block / words,
                    },
                    part: Frame {
                        stack: Stack::new(Blocks::default()),
                        words,
                        count: 1,
                    },
                },
            })
        });
        Rows {
            rows: spread.into_iter().chain(fast).collect(),
        }
        .text()
    }
}

/// The rows that time a frame's `enter` and `leave`: of an empty frame,
/// and of frames that filled 1, 10 and 100 blocks, both the whole round
/// trip and `leave` alone, per block; `leave` alone is timed over 100
/// blocks at a time, in as many frames as that takes.
///
/// Each block is filled by one allocation as large as a block holds,
/// so the round trip is `enter`, one `pop` and header stamp per block,
/// and the `leave` that pushes them all back.
struct Leaves {
    block: usize,
}

impl Text for Leaves {
    fn text(&mut self) -> String {
        let empty: Box<dyn Text> = Box::new(Timing {
            what: "`enter` + `leave` of an empty frame",
            size: "0 × 4 KB".to_owned(),
            ops: 1,
            work: Frame {
                stack: Stack::new(Blocks::default()),
                words: self.block,
                count: 0,
            },
        });
        let trip = [1, 10, 100].map(|blocks| -> Box<dyn Text> {
            Box::new(Timing {
                what: "`enter` + fill + `leave`, per block",
                size: format!("{blocks} × 4 KB"),
                ops: blocks,
                work: Frame {
                    stack: Stack::new(Blocks::default()),
                    words: self.block,
                    count: blocks,
                },
            })
        });
        let alone = [1, 10, 100].map(|blocks| -> Box<dyn Text> {
            Box::new(Timing {
                what: "`leave` alone, per block",
                size: format!("{blocks} × 4 KB"),
                ops: blocks * (100 / blocks),
                work: Leave {
                    stack: Stack::new(Blocks::default()),
                    words: self.block,
                    blocks,
                    frames: 100 / blocks,
                },
            })
        });
        Rows {
            rows: std::iter::once(empty).chain(trip).chain(alone).collect(),
        }
        .text()
    }
}

/// The whole results file: how it was made, on what machine and build,
/// the table, and eoc's numbers beside it.
struct Results {
    machine: Vec<(&'static str, Shell)>,
    profile: &'static str,
    capacity: usize,
    rows: Rows,
}

impl Text for Results {
    fn text(&mut self) -> String {
        format!(
            "# Bench\n\n\
             Written by `cargo run --release --bin bench`; every run rewrites it.\n\n\
             | machine | |\n|---|---|\n{}\n| profile | {} |\n\n\
             Each row: a warm-up that doubles the repetitions until the timed part\n\
             of one run takes 10 ms, then 15 samples of that many repetitions. `median` is the\n\
             median of the samples in ns per operation, `spread` is the fastest\n\
             and the slowest sample, `ops` is the operations timed per sample.\n\
             Allocations run in the youngest frame: frame 1, or for `leave` alone\n\
             each of the nested frames while it is the youngest. A block holds {}\n\
             words after its header, measured by this run.\n\n\
             | what | size | median ns | spread ns | ops |\n|---|---|---|---|---|\n{}\n\n\
             eoc, from README §7: 163 ns per object, 45 to allocate and 117 to\n\
             collect.\n",
            self.machine
                .iter_mut()
                .map(|(name, shell)| format!("| {name} | {} |", shell.text()))
                .collect::<Vec<_>>()
                .join("\n"),
            self.profile,
            self.capacity,
            self.rows.text()
        )
    }
}

fn main() {
    let block = Capacity {
        stack: Stack::new(Blocks::default()),
    }
    .amount();
    let text = Results {
        machine: vec![
            (
                "CPU",
                if cfg!(target_os = "macos") {
                    Shell {
                        program: "sysctl",
                        args: &["-n", "machdep.cpu.brand_string"],
                    }
                } else {
                    Shell {
                        program: "sh",
                        args: &["-c", "grep -m1 'model name' /proc/cpuinfo | cut -d: -f2"],
                    }
                },
            ),
            (
                "OS",
                Shell {
                    program: "uname",
                    args: &["-srm"],
                },
            ),
            (
                "load",
                if cfg!(target_os = "macos") {
                    Shell {
                        program: "sysctl",
                        args: &["-n", "vm.loadavg"],
                    }
                } else {
                    Shell {
                        program: "cat",
                        args: &["/proc/loadavg"],
                    }
                },
            ),
            (
                "rustc",
                Shell {
                    program: "rustc",
                    args: &["-V"],
                },
            ),
        ],
        profile: if cfg!(debug_assertions) {
            "debug, so these numbers mean nothing"
        } else {
            "release"
        },
        capacity: block,
        rows: Rows {
            rows: vec![
                Box::new(Timing {
                    what: "pool `pop` + `push`",
                    size: "1 × 4 KB".to_owned(),
                    ops: 1,
                    work: Cycle {
                        pool: Blocks::default(),
                    },
                }),
                Box::new(Allocs { block }),
                Box::new(Leaves { block }),
            ],
        },
    }
    .text();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("bench");
    std::fs::create_dir_all(&path)
        .unwrap_or_else(|error| panic!("cannot create {}: {error}", path.display()));
    std::fs::write(path.join("results.md"), text)
        .unwrap_or_else(|error| panic!("cannot write {}: {error}", path.display()));
}

#[cfg(test)]
mod tests {
    use super::{Capacity, Difference, Quantity, Rows, Shell, Text, Timing, Work};
    use eorm::frames::{Frames, Stack};
    use eorm::pool::{Blocks, Pool};
    use std::time::Duration;

    /// Work that takes the same made-up time on every repetition.
    struct Fixed {
        nanos: u64,
    }

    impl Work for Fixed {
        fn run(&mut self, reps: usize) -> Duration {
            Duration::from_nanos(self.nanos * u64::try_from(reps).expect("reps fit in u64"))
        }
    }

    /// Text that is always the same line.
    struct Line {
        line: &'static str,
    }

    impl Text for Line {
        fn text(&mut self) -> String {
            self.line.to_owned()
        }
    }

    #[test]
    fn difference_takes_the_part_out_of_the_whole() {
        assert_eq!(
            Difference {
                whole: Fixed { nanos: 731 },
                part: Fixed { nanos: 94 },
            }
            .run(13),
            Duration::from_nanos(637 * 13),
            "the difference does not take the part out of the whole"
        );
    }

    #[test]
    fn rows_put_each_row_on_its_own_line() {
        assert_eq!(
            Rows {
                rows: vec![
                    Box::new(Line { line: "| ä | 7 |" }),
                    Box::new(Line { line: "| q | 19 |" }),
                ],
            }
            .text(),
            "| ä | 7 |\n| q | 19 |",
            "the rows do not come one per line"
        );
    }

    #[test]
    fn shell_gives_the_output_of_its_command_without_the_spaces_around() {
        assert_eq!(
            Shell {
                program: "echo",
                args: &["  x7Ü q "],
            }
            .text(),
            "x7Ü q",
            "the shell does not give the trimmed output of its command"
        );
    }

    #[test]
    fn timing_reports_the_time_of_one_operation() {
        assert_eq!(
            Timing {
                what: "ü",
                size: "7 q".to_owned(),
                ops: 7,
                work: Fixed { nanos: 23_456_789 },
            }
            .text(),
            "| ü | 7 q | 3350969.86 | 3350969.86–3350969.86 | 7 |",
            "the timing does not report the time of one operation"
        );
    }

    #[test]
    fn capacity_words_fill_a_block_to_its_end() {
        let mut stack = Stack::new(Blocks::new(3));
        let words = Capacity {
            stack: Stack::new(Blocks::new(5)),
        }
        .amount();
        stack.enter();
        assert!(
            (stack.alloc(1, words).addr().get() + words * 8).is_multiple_of(Blocks::SIZE),
            "the measured capacity does not fill a block to its end"
        );
    }
}
