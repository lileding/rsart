use libc::{c_int, c_void};
use mio::event::Source;
use mio::unix::SourceFd;
use mio::{Interest, Registry, Token};
use std::io;
use std::mem::MaybeUninit;
use std::os::unix::io::{AsRawFd, RawFd};
use std::time::Duration;

#[cfg(not(target_os = "linux"))]
compile_error!("timerfd is a linux specific feature");

//
//
// TimerFd
//
//

/// A timerfd which can be used to create mio-compatible
/// timers on linux targets.
pub(crate) struct TimerFd {
    fd: c_int,
}

impl TimerFd {
    /// Create a new timerfd using the given clock; if you're not sure
    /// what clock to use read timerfd(7) for more details, or know
    /// that `ClockId::Monotonic` is a good default for most programs.
    pub(crate) fn new(clockid: ClockId) -> io::Result<Self> {
        let flags = libc::TFD_NONBLOCK | libc::TFD_CLOEXEC;
        Self::create(clockid.into(), flags)
    }

    /// Set a single timeout to occur after the specified duration.
    pub(crate) fn set_timeout(&mut self, timeout: &Duration) -> io::Result<()> {
        // this is overflow safe unless the timeout is > sizeof(long) seconds,
        // which is a healthy ~68 years for 32 bit or ~2 billion years for 64 bit.
        let new_value = libc::itimerspec {
            it_interval: libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            },
            it_value: libc::timespec {
                tv_sec: timeout.as_secs() as libc::time_t,
                tv_nsec: timeout.subsec_nanos() as libc::c_long,
            },
        };
        self.settime(0, &new_value).map(|_old_value| ())
    }

    /// Set a timeout to occur at each interval of the
    /// specified duration from this point in time forward.
    #[allow(dead_code)]
    pub(crate) fn set_timeout_interval(&mut self, timeout: &Duration) -> io::Result<()> {
        // this is overflow safe unless the timoeout is > ~292 billion years.
        let new_value = libc::itimerspec {
            it_interval: libc::timespec {
                tv_sec: timeout.as_secs() as libc::time_t,
                tv_nsec: timeout.subsec_nanos() as libc::c_long,
            },
            it_value: libc::timespec {
                tv_sec: timeout.as_secs() as libc::time_t,
                tv_nsec: timeout.subsec_nanos() as libc::c_long,
            },
        };
        self.settime(0, &new_value).map(|_old_value| ())
    }

    /// Unset any existing timeouts on the timer,
    /// making this timerfd inert until rearmed.
    #[allow(dead_code)]
    pub(crate) fn disarm(&mut self) -> io::Result<()> {
        self.set_timeout(&Duration::from_secs(0))
    }

    /// Read the timerfd to reset the readability of the timerfd,
    /// and allow determining how many times the timer has elapsed
    /// since the last read. This should usually be read after
    /// any wakeups caused by this timerfd, as reading the timerfd
    /// is important to reset the readability of the timerfd.
    ///
    /// Failing to call this after this timerfd causes a wakeup
    /// will result in immediately re-waking on this timerfd if
    /// level polling, or never re-waking if edge polling.
    #[allow(dead_code)]
    pub(crate) fn read(&self) -> io::Result<u64> {
        let mut buf = [0u8; 8];
        let ret = unsafe { libc::read(self.fd, buf.as_mut_ptr() as *mut c_void, buf.len()) };
        if ret == 8 {
            Ok(u64::from_ne_bytes(buf))
        } else if ret == -1 {
            let errno = unsafe { *libc::__errno_location() };
            if errno == libc::EAGAIN {
                Ok(0)
            } else {
                Err(io::Error::from_raw_os_error(errno))
            }
        } else {
            panic!("reading a timerfd should never yield {} bytes", ret);
        }
    }

    /// Wrapper of `timerfd_create` from timerfd_create(7); For
    /// most users it's probably easier to use the `TimerFd::new`.
    ///
    /// Note that this library may cause the thread to block when
    /// `TimerFd::read` is called if the `TFD_NONBLOCK` flag is
    /// not included in the flags.
    pub(crate) fn create(clockid: c_int, flags: c_int) -> io::Result<Self> {
        let fd = unsafe { libc::timerfd_create(clockid, flags) };
        if fd == -1 {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self { fd })
        }
    }

    /// Wrapper of `timerfd_settime` from timerfd_create(7); For most
    /// users it's probably easier to use the `TimerFd::set_timeout` or
    /// the `TimerFd::set_timeout_interval` functions.
    pub(crate) fn settime(
        &mut self,
        flags: c_int,
        new_value: &libc::itimerspec,
    ) -> io::Result<libc::itimerspec> {
        let mut old_spec_mem = MaybeUninit::<libc::itimerspec>::uninit();
        let ret =
            unsafe { libc::timerfd_settime(self.fd, flags, new_value, old_spec_mem.as_mut_ptr()) };
        if ret == -1 {
            Err(io::Error::last_os_error())
        } else {
            let old_spec = unsafe { old_spec_mem.assume_init() };
            Ok(old_spec)
        }
    }

    /// Wrapper of `timerfd_gettime` from timerfd_create(7)
    #[allow(dead_code)]
    pub(crate) fn gettime(&self) -> io::Result<libc::itimerspec> {
        let mut old_spec_mem = MaybeUninit::<libc::itimerspec>::uninit();
        let ret = unsafe { libc::timerfd_gettime(self.fd, old_spec_mem.as_mut_ptr()) };
        if ret == -1 {
            Err(io::Error::last_os_error())
        } else {
            let old_spec = unsafe { old_spec_mem.assume_init() };
            Ok(old_spec)
        }
    }
}

impl AsRawFd for TimerFd {
    fn as_raw_fd(&self) -> RawFd {
        self.fd
    }
}

impl Source for TimerFd {
    fn register(
        &mut self,
        registry: &Registry,
        token: Token,
        interest: Interest,
    ) -> io::Result<()> {
        SourceFd(&self.fd).register(registry, token, interest)
    }

    fn reregister(
        &mut self,
        registry: &Registry,
        token: Token,
        interest: Interest,
    ) -> io::Result<()> {
        SourceFd(&self.fd).reregister(registry, token, interest)
    }

    fn deregister(&mut self, registry: &Registry) -> io::Result<()> {
        SourceFd(&self.fd).deregister(registry)
    }
}

impl Drop for TimerFd {
    fn drop(&mut self) {
        let _ = unsafe { libc::close(self.fd) };
    }
}

//
//
// ClockId
//
//

/// Clock used to mark the progress of the timer. timerfd_create(7)
#[allow(dead_code)]
#[derive(Copy, Clone)]
pub(crate) enum ClockId {
    RealTime,
    Monotonic,
    BootTime,
    RealTimeAlarm,
    BootTimeAlarm,
}

impl Into<c_int> for ClockId {
    fn into(self) -> c_int {
        match self {
            ClockId::RealTime => libc::CLOCK_REALTIME,
            ClockId::Monotonic => libc::CLOCK_MONOTONIC,
            ClockId::BootTime => libc::CLOCK_BOOTTIME,
            ClockId::RealTimeAlarm => libc::CLOCK_REALTIME_ALARM,
            ClockId::BootTimeAlarm => libc::CLOCK_BOOTTIME_ALARM,
        }
    }
}

//
//
// Testing
//
//

#[cfg(test)]
mod test {
    use super::*;
    use mio::{Events, Poll};

    const TOK: Token = Token(0);
    const TIMEOUT: Duration = Duration::from_millis(60);

    #[test]
    fn single_timeout() {
        let mut poll = Poll::new().unwrap();
        let mut events = Events::with_capacity(1024);
        let mut timer = TimerFd::new(ClockId::Monotonic).unwrap();
        timer.set_timeout(&TIMEOUT).unwrap();
        poll.registry()
            .register(&mut timer, TOK, Interest::READABLE)
            .unwrap();

        // timer should not elapse before the timeout
        poll.poll(&mut events, Some(TIMEOUT / 2)).unwrap();
        assert!(events.is_empty());
        assert!(timer.read().unwrap() == 0);

        // timer should elapse after its timeout has passed
        poll.poll(&mut events, Some(TIMEOUT)).unwrap();
        assert!(!events.is_empty());
        assert!(timer.read().unwrap() == 1);

        // timer should not elapse again without a rearm
        poll.poll(&mut events, Some(TIMEOUT)).unwrap();
        assert!(events.is_empty());
        assert!(timer.read().unwrap() == 0);
    }

    #[test]
    fn disarm_rearm_single_timeout() {
        let mut poll = Poll::new().unwrap();
        let mut events = Events::with_capacity(1024);
        let mut timer = TimerFd::new(ClockId::Monotonic).unwrap();
        timer.set_timeout(&TIMEOUT).unwrap();
        poll.registry()
            .register(&mut timer, TOK, Interest::READABLE)
            .unwrap();

        // timer should not elapse before the timeout
        poll.poll(&mut events, Some(TIMEOUT / 2)).unwrap();
        assert!(events.is_empty());
        assert!(timer.read().unwrap() == 0);

        // timer should not elapse after its first
        // timeout has passed if we disarm the timer.
        timer.disarm().unwrap();
        poll.poll(&mut events, Some(TIMEOUT)).unwrap();
        assert!(events.is_empty());
        assert!(timer.read().unwrap() == 0);

        // timer should elapse after the rearmed timeout
        timer.set_timeout(&TIMEOUT).unwrap();
        poll.poll(&mut events, Some(TIMEOUT * 2)).unwrap();
        assert!(!events.is_empty());
        assert!(timer.read().unwrap() == 1);
    }

    #[test]
    fn timeout_interval() {
        let mut poll = Poll::new().unwrap();
        let mut events = Events::with_capacity(1024);
        let mut timer = TimerFd::new(ClockId::Monotonic).unwrap();
        timer.set_timeout_interval(&TIMEOUT).unwrap();
        poll.registry()
            .register(&mut timer, TOK, Interest::READABLE)
            .unwrap();

        // timer should not elapse before the timeout
        poll.poll(&mut events, Some(TIMEOUT / 2)).unwrap();
        assert!(events.is_empty());
        assert!(timer.read().unwrap() == 0);

        // timer should elapsed after its timeout
        poll.poll(&mut events, Some(TIMEOUT)).unwrap();
        assert!(!events.is_empty());
        assert!(timer.read().unwrap() == 1);

        // timer should elapse again after another timeout
        poll.poll(&mut events, Some(TIMEOUT * 2)).unwrap();
        assert!(!events.is_empty());
        assert!(timer.read().unwrap() == 1);
    }

    #[test]
    fn disarm_rearm_timeout_interval() {
        let mut poll = Poll::new().unwrap();
        let mut events = Events::with_capacity(1024);
        let mut timer = TimerFd::new(ClockId::Monotonic).unwrap();
        timer.set_timeout_interval(&TIMEOUT).unwrap();
        poll.registry()
            .register(&mut timer, TOK, Interest::READABLE)
            .unwrap();

        // timer should not elapse before the timeout
        poll.poll(&mut events, Some(TIMEOUT / 2)).unwrap();
        assert!(events.is_empty());
        assert!(timer.read().unwrap() == 0);

        // timer should not elapse after its first
        // timeout has passed if we disarm the timer,
        timer.disarm().unwrap();
        poll.poll(&mut events, Some(TIMEOUT)).unwrap();
        assert!(events.is_empty());
        assert!(timer.read().unwrap() == 0);

        // timer should elapse after the rearmed timeout
        timer.set_timeout_interval(&TIMEOUT).unwrap();
        poll.poll(&mut events, Some(TIMEOUT + (TIMEOUT / 2)))
            .unwrap();
        assert!(!events.is_empty());
        assert!(timer.read().unwrap() == 1);

        // timer should elapse after the rearmed timeout
        timer.set_timeout_interval(&TIMEOUT).unwrap();
        poll.poll(&mut events, Some(TIMEOUT + (TIMEOUT / 2)))
            .unwrap();
        assert!(!events.is_empty());
        assert!(timer.read().unwrap() == 1);
    }

    #[test]
    fn deregister_and_drop() {
        let mut poll = Poll::new().unwrap();
        let mut events = Events::with_capacity(1024);

        let mut timer_one = TimerFd::new(ClockId::Monotonic).unwrap();
        timer_one.set_timeout(&Duration::from_millis(32)).unwrap();
        poll.registry()
            .register(&mut timer_one, Token(1), Interest::READABLE)
            .unwrap();
        let mut timer_two = TimerFd::new(ClockId::Monotonic).unwrap();
        timer_two.set_timeout(&Duration::from_millis(64)).unwrap();
        poll.registry()
            .register(&mut timer_two, Token(2), Interest::READABLE)
            .unwrap();

        // ensure we can deregister and drop a previously
        // registered timer without any issue.
        poll.poll(&mut events, Some(Duration::from_millis(5)))
            .unwrap();
        assert!(events.is_empty());
        poll.registry().deregister(&mut timer_one).unwrap();
        std::mem::drop(timer_one);

        poll.poll(&mut events, Some(Duration::from_millis(500)))
            .unwrap();
        assert!(!events.is_empty());
        for event in events.iter() {
            match event.token() {
                Token(1) => panic!(),
                Token(2) => {}
                _ => panic!(),
            }
        }
    }

    #[test]
    fn multiple_timers() {
        use std::time::Instant;

        let deadline = Instant::now() + Duration::from_millis(330);
        let mut count_one = 0;
        let mut count_two = 0;
        let mut count_three = 0;
        let mut count_four = 0;

        let mut poll = Poll::new().unwrap();
        let mut events = Events::with_capacity(1024);

        // timer one should tick once at 10ms
        let mut timer_one = TimerFd::new(ClockId::Monotonic).unwrap();
        timer_one.set_timeout(&Duration::from_millis(100)).unwrap();
        poll.registry()
            .register(&mut timer_one, Token(1), Interest::READABLE)
            .unwrap();

        // timer two should tick each 10ms
        let mut timer_two = TimerFd::new(ClockId::Monotonic).unwrap();
        timer_two
            .set_timeout_interval(&Duration::from_millis(100))
            .unwrap();
        poll.registry()
            .register(&mut timer_two, Token(2), Interest::READABLE)
            .unwrap();

        // timer three should tick once at 20ms
        let mut timer_three = TimerFd::new(ClockId::Monotonic).unwrap();
        timer_three
            .set_timeout(&Duration::from_millis(200))
            .unwrap();
        poll.registry()
            .register(&mut timer_three, Token(3), Interest::READABLE)
            .unwrap();

        // timer four should tick each 30ms
        let mut timer_four = TimerFd::new(ClockId::Monotonic).unwrap();
        timer_four
            .set_timeout_interval(&Duration::from_millis(300))
            .unwrap();
        poll.registry()
            .register(&mut timer_four, Token(4), Interest::READABLE)
            .unwrap();

        loop {
            poll.poll(&mut events, Some(deadline - Instant::now()))
                .unwrap();
            if events.is_empty() {
                break;
            }
            for event in events.iter() {
                match event.token() {
                    Token(1) => {
                        let _ = timer_one.read();
                        count_one += 1;
                        if count_one == 1 {
                            assert!(count_two <= 1);
                            assert!(count_three == 0);
                            assert!(count_four == 0);
                            timer_one.set_timeout(&Duration::from_millis(150)).unwrap();
                        }
                    }
                    Token(2) => {
                        let _ = timer_two.read();
                        count_two += 1;
                        assert!(count_two == 1 || count_two == 2);
                        // only let this timer tick twice
                        if count_two >= 2 {
                            timer_two.disarm().unwrap();
                        }
                        // check ticks on other clocks make sense
                        if count_two == 1 {
                            assert!(count_one <= 1);
                            assert!(count_three == 0);
                            assert!(count_four == 0);
                        } else if count_two == 2 {
                            assert!(count_one == 1);
                            assert!(count_three <= 1);
                            assert!(count_four == 0);
                        }
                    }
                    Token(3) => {
                        let _ = timer_three.read();
                        count_three += 1;
                        assert!(count_one == 1);
                        assert!(count_two == 1 || count_two == 2);
                        assert!(count_three == 1);
                        assert!(count_four == 0);
                    }
                    Token(4) => {
                        let _ = timer_four.read();
                        count_four += 1;
                        assert!(count_one == 2);
                        assert!(count_two == 2);
                        assert!(count_three == 1);
                        assert!(count_four == 1);
                    }
                    _ => unreachable!(),
                }
            }
        }

        assert!(count_one == 2);
        assert!(count_two == 2);
        assert!(count_three == 1);
        assert!(count_four == 1);
    }
}
