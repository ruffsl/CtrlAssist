//! Asking the kernel for real-time scheduling.

#![expect(
    unsafe_code,
    reason = "the scheduler call has no safe wrapper in the crates used here"
)]

/// Puts the calling thread under first-in-first-out real-time scheduling
/// at `priority`, and says whether the kernel allowed it. A machine that
/// doesn't grant its users real-time priority refuses, and the thread
/// then carries on as it was.
///
/// The priority is meant to be low: above every ordinary thread, which is
/// all the input path needs, and below the audio and kernel threads that
/// use the higher ones. The kernel's own limit on how much of each second
/// real-time threads may run is what protects the machine from a thread
/// that never sleeps.
pub(crate) fn request(priority: i32) -> bool {
    let param = libc::sched_param {
        sched_priority: priority,
    };
    // SAFETY: `param` is a valid `sched_param` for the length of the
    // call, and a thread id of zero means the calling thread. The reset
    // flag keeps a child process from inheriting the policy.
    let result = unsafe {
        libc::sched_setscheduler(
            0,
            libc::SCHED_FIFO | libc::SCHED_RESET_ON_FORK,
            &raw const param,
        )
    };
    result == 0
}
