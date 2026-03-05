use crate::environment::Environment;
use crate::cpu::Cpu;

pub fn objc_personality_v0(
    _env: &mut Environment,
    cpu: &mut Cpu,
) -> u32 {
    let state = cpu.reg(0);
    let exception_object = cpu.reg(1);

    log!(
        "objc_personality_v0 called: state={:#x}, exception={:#x}",
        state,
        exception_object
    );

    // ARM EHABI:
    // 0 = _URC_CONTINUE_UNWIND
    0
}
