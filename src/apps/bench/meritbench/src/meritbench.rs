#![no_std]

use m3::{
    build_vmsg,
    col::String,
    errors::{Code, Error},
    io::{Read, Write},
    kif::{self, Perm},
    mem::{MsgBuf, VirtAddr},
    serialize::M3Deserializer,
    tcu::{self, EpId},
    time::{CycleDuration, CycleInstant, Duration, Profiler, Results, Runner, TimeDuration},
    vfs::{FileMode, FileRef, GenericFile, OpenFlags, VFS},
    test::{DefaultWvTester, WvTester},
    tiles::{Activity, ActivityArgs, ChildActivity, RunningActivity, Tile},
    cap::Selector,
    com::{recv_msg, RecvCap, RecvGate, SGateArgs, SendGate},
    format, println, reply_vmsg, send_vmsg, wv_assert_eq, wv_assert_ok, wv_perf, wv_run_test,
};
const MSG_ORD: u32 = 8;

const WARMUP: u64 = 50;
const RUNS: u64 = 1000;

fn wait_for_rpl(rep: EpId, rcv_buf: VirtAddr) -> Result<(), Error> {
    loop {
        if let Some(off) = tcu::TCU::fetch_msg(rep) {
            let msg = tcu::TCU::offset_to_msg(rcv_buf, off);
            let mut de = M3Deserializer::new(msg.as_words());
            let res: Code = de.pop()?;
            tcu::TCU::ack_msg(rep, off)?;
            return Result::from(res);
        }
    }
}

fn noop_syscall(rbuf: VirtAddr) {
    let mut msg = MsgBuf::borrow_def();
    build_vmsg!(msg, kif::syscalls::Operation::Noop, kif::syscalls::Noop {});
    tcu::TCU::send(
        tcu::FIRST_USER_EP + tcu::SYSC_SEP_OFF,
        &msg,
        0,
        tcu::FIRST_USER_EP + tcu::SYSC_REP_OFF,
    )
    .unwrap();
    wait_for_rpl(tcu::FIRST_USER_EP + tcu::SYSC_REP_OFF, rbuf).unwrap();
}

#[inline(never)]
fn bench_custom_noop_syscall(profiler: &Profiler) -> Results<CycleDuration> {
    let (rbuf, _) = Activity::own().tile_desc().rbuf_std_space();
    profiler.run::<CycleInstant, _>(|| {
        noop_syscall(rbuf);
    })
}

#[inline(never)]
fn bench_m3_noop_syscall(profiler: &Profiler) -> Results<CycleDuration> {
    profiler.run::<CycleInstant, _>(|| {
        m3::syscalls::noop().unwrap();
    })
}

#[inline(never)]
fn bench_tlb_insert(profiler: &Profiler) -> Results<CycleDuration> {
    let sample_addr = VirtAddr::from(profiler as *const _);
    profiler.run::<CycleInstant, _>(|| {
        tcu::TCU::handle_xlate_fault(sample_addr, Perm::R);
    })
}

#[inline(never)]
fn bench_os_call(profiler: &Profiler) -> Results<CycleDuration> {
    profiler.run::<CycleInstant, _>(|| {
        m3::tmif::noop().unwrap();
    })
}

const READ_STR_LEN: usize = 1024 * 1024;
const WRITE_STR_LEN: usize = 8 * 1024;

#[inline(never)]
fn bench_m3fs_read(profiler: &Profiler) -> Results<CycleDuration> {
    let mut file = VFS::open("/new-file.txt", OpenFlags::CREATE | OpenFlags::RW).unwrap();
    let content: String = (0..READ_STR_LEN).map(|_| "a").collect();
    write!(file, "{}", content).unwrap();

    let res = profiler.run::<CycleInstant, _>(|| {
        let _content = file.read_to_string().unwrap();
    });

    VFS::unlink("/new-file.txt").unwrap();
    res
}

struct WriteBenchmark {
    file: FileRef<GenericFile>,
    content: String,
}

impl WriteBenchmark {
    fn new() -> WriteBenchmark {
        WriteBenchmark {
            file: VFS::open("/new-file.txt", OpenFlags::CREATE | OpenFlags::W).unwrap(),
            content: (0..WRITE_STR_LEN).map(|_| "a").collect(),
        }
    }
}

impl Drop for WriteBenchmark {
    fn drop(&mut self) {
        VFS::unlink("/new-file.txt").unwrap();
    }
}

impl Runner for WriteBenchmark {
    fn run(&mut self) {
        self.file.write_all(self.content.as_bytes()).unwrap();
    }

    fn post(&mut self) {
        self.file.borrow().truncate(0).unwrap();
    }
}

#[inline(never)]
fn bench_m3fs_write(profiler: &Profiler) -> Results<CycleDuration> {
    profiler.runner::<CycleInstant, _>(&mut WriteBenchmark::new())
}

#[inline(never)]
fn bench_m3fs_meta(profiler: &Profiler) -> Results<CycleDuration> {
    profiler.run::<CycleInstant, _>(|| {
        VFS::mkdir("/new-dir", FileMode::from_bits(0o755).unwrap()).unwrap();
        let _ = VFS::stat("/new-dir").unwrap();
        {
            let _ = VFS::open("/new-dir/new-file", OpenFlags::CREATE).unwrap();
        }
        {
            let mut file = VFS::open("/new-dir/new-file", OpenFlags::W).unwrap();
            write!(file, "test").unwrap();
        }
        {
            let mut file = VFS::open("/new-dir/new-file", OpenFlags::R).unwrap();
            let _ = file.read_to_string().unwrap();
            let _ = VFS::stat("/new-dir/new-file").unwrap();
        }

        VFS::link("/new-dir/new-file", "/new-link").unwrap();
        VFS::rename("/new-link", "/new-blink").unwrap();
        let _ = VFS::stat("/new-blink");
        VFS::unlink("/new-blink").unwrap();
        VFS::unlink("/new-dir/new-file").unwrap();
        VFS::rmdir("/new-dir").unwrap();
    })
}

fn print_summary<T: Duration + Clone>(name: &str, res: &Results<T>) {
    println!("{}: {}", name, res);
}

fn pingpong_with_multiple(t: &mut dyn WvTester) {
    if !Activity::own().tile_desc().has_virtmem() {
        println!("No virtual memory; skipping remote multi IPC test");
        return;
    }

    let tile = wv_assert_ok!(Tile::get("compat"));
    // use long time slices for childs (minimize timer interrupts)
    wv_assert_ok!(tile.set_quota(
        TimeDuration::from_secs(1),
        tile.quota().unwrap().page_tables().remaining(),
    ));

    // split time quota between childs
    let cur_quota = wv_assert_ok!(tile.quota()).time().total();
    let tile1 = wv_assert_ok!(tile.derive(None, Some(cur_quota / 2), None));
    let tile2 = wv_assert_ok!(tile.derive(None, Some(cur_quota / 2), None));

    let mut act1 = wv_assert_ok!(ChildActivity::new_with(tile1, ActivityArgs::new("recv1")));
    let mut act2 = wv_assert_ok!(ChildActivity::new_with(tile2, ActivityArgs::new("recv2")));

    let rgate1 = wv_assert_ok!(RecvCap::new(MSG_ORD, MSG_ORD));
    let rgate2 = wv_assert_ok!(RecvCap::new(MSG_ORD, MSG_ORD));

    wv_assert_ok!(act1.delegate_obj(rgate1.sel()));
    wv_assert_ok!(act2.delegate_obj(rgate2.sel()));

    act1.data_sink().push(rgate1.sel());
    act2.data_sink().push(rgate2.sel());

    let func = || {
        let mut t = DefaultWvTester::default();
        let rgate_sel: Selector = Activity::own().data_source().pop().unwrap();
        let rgate = RecvGate::new_bind(rgate_sel).unwrap();
        for _ in 0..(RUNS + WARMUP) / 2 {
            let mut msg = wv_assert_ok!(recv_msg(&rgate));
            wv_assert_eq!(t, msg.pop::<u64>(), Ok(0));
            wv_assert_ok!(reply_vmsg!(msg, 0u64));
        }
        Ok(())
    };

    let act1 = wv_assert_ok!(act1.run(func));
    let act2 = wv_assert_ok!(act2.run(func));

    let prof = Profiler::default().repeats(RUNS).warmup(WARMUP);

    let sgate1 = wv_assert_ok!(SendGate::new_with(SGateArgs::new(&rgate1).credits(1)));
    let sgate2 = wv_assert_ok!(SendGate::new_with(SGateArgs::new(&rgate2).credits(1)));
    let reply_gate = RecvGate::def();

    let mut count = 0;
    wv_perf!(
        "remote multi pingpong with (1 * u64) msgs",
        prof.run::<CycleInstant, _>(|| {
            // alternate between bothr receivers to ensure that we always need a context switch on
            // the other tile
            if count % 2 == 0 {
                wv_assert_ok!(send_vmsg!(&sgate1, reply_gate, 0u64));
            }
            else {
                wv_assert_ok!(send_vmsg!(&sgate2, reply_gate, 0u64));
            }

            let mut reply = wv_assert_ok!(recv_msg(reply_gate));
            wv_assert_eq!(t, reply.pop::<u64>(), Ok(0));

            count += 1;
        })
    );

    wv_assert_eq!(t, act1.wait(), Ok(Code::Success));
    wv_assert_eq!(t, act2.wait(), Ok(Code::Success));
}

#[no_mangle]
pub fn main() -> Result<(), Error> {
    let profiler = Profiler::default().warmup(10).repeats(100);
    let mut tester = DefaultWvTester::default();
    
    wv_run_test!(tester, pingpong_with_multiple);

    Ok(())
}
