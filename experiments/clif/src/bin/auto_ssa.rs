// Вариант 2: та же функция sum_sq, но SSA строится автоматически.
//
// Вариант 1 (main.rs) строил SSA вручную: параметры блоков, явные
// аргументы jump/brif. Здесь используется `FunctionBuilder::use_var`
// и `def_var` с `Variable` — компилятор Cranelift сам разводит
// определения по блокам и ставит нужные аргументы.
//
// Вопрос, который это проверяет: сколько кода экономит автоматический
// SSA и насколько это меняет оценку миграции.

use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::immediates::Imm64;
use cranelift_codegen::ir::types;
use cranelift_codegen::ir::{AbiParam, InstBuilder, MemFlagsData};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{default_libcall_names, Linkage, Module};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut flag_builder = settings::builder();
    flag_builder.set("opt_level", "speed")?;
    let flags = settings::Flags::new(flag_builder);

    let jit_builder = JITBuilder::with_flags(
        &[("opt_level", "speed")],
        default_libcall_names(),
    )?;
    let mut module = JITModule::new(jit_builder);

    // ============ sum_sq(a: *i64) -> i64 ============
    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(types::I64));
    sig.returns.push(AbiParam::new(types::I64));
    let sum_sq_id = module.declare_function("sum_sq", Linkage::Export, &sig)?;

    let mut ctx = module.make_context();
    ctx.func.signature = sig.clone();
    let mut fb_ctx = FunctionBuilderContext::new();
    {
        let mut b = FunctionBuilder::new(&mut ctx.func, &mut fb_ctx);

        let entry = b.create_block();
        b.append_block_params_for_function_params(entry);
        b.seal_block(entry);
        b.switch_to_block(entry);

        let a = b.block_params(entry)[0];

        // Переменные уровня языка — обычные переменные, как в исходнике.
        let total = b.declare_var(types::I64);
        let i = b.declare_var(types::I64);

        let zero = b.ins().iconst(types::I64, 0);
        b.def_var(total, zero);
        b.def_var(i, zero);

        let header = b.create_block();
        let body = b.create_block();
        let exit = b.create_block();

        b.ins().jump(header, &[]);

        // --- заголовок цикла ---
        b.switch_to_block(header);
        let hi = b.use_var(i);
        let cond = b.ins().icmp_imm_u(IntCC::UnsignedLessThan, hi, Imm64::new(4));
        b.ins().brif(cond, body, &[], exit, &[]);

        // --- тело цикла ---
        b.switch_to_block(body);
        let bi = b.use_var(i);
        let btotal = b.use_var(total);
        let off = b.ins().imul_imm_u(bi, 8);
        let addr = b.ins().iadd(a, off);
        let val = b.ins().load(types::I64, MemFlagsData::trusted(), addr, 0);
        let sq = b.ins().imul(val, val);
        let total2 = b.ins().iadd(btotal, sq);
        let i2 = b.ins().iadd_imm_u(bi, 1);
        b.def_var(total, total2);
        b.def_var(i, i2);
        b.ins().jump(header, &[]);

        // --- выход ---
        b.switch_to_block(exit);
        let etotal = b.use_var(total);
        b.ins().return_(&[etotal]);

        b.seal_all_blocks();
        // finalize() обязателен: он проверяет, что все блоки запечатаны
        // и заполнены, и сбрасывает состояние контекста.
        b.finalize(module.target_config());
    }

    println!("=== CLIF для sum_sq (автоматический SSA) ===");
    println!("{}", ctx.func.display());
    std::fs::write("/tmp/auto_ssa.clif", format!("{}", ctx.func.display())).ok();

    module.define_function(sum_sq_id, &mut ctx)?;

    // ============ main ============
    let mut sig2 = module.make_signature();
    sig2.returns.push(AbiParam::new(types::I64));
    let main_id = module.declare_function("main", Linkage::Export, &sig2)?;

    let mut ctx2 = module.make_context();
    ctx2.func.signature = sig2.clone();
    let mut fb_ctx2 = FunctionBuilderContext::new();
    {
        let mut b = FunctionBuilder::new(&mut ctx2.func, &mut fb_ctx2);
        let entry = b.create_block();
        b.append_block_params_for_function_params(entry);
        b.seal_block(entry);
        b.switch_to_block(entry);

        let ss = b.create_sized_stack_slot(
            cranelift_codegen::ir::stackslot::StackSlotData::new(
                cranelift_codegen::ir::stackslot::StackSlotKind::ExplicitSlot,
                32,
                3,
            ),
        );
        for (idx, n) in [1i64, 2, 3, 4].iter().enumerate() {
            let c = b.ins().iconst(types::I64, *n);
            b.ins().stack_store(types::I64, c, ss, (idx * 8) as i32);
        }

        let addr = b.ins().stack_addr(types::I64, ss, 0);
        let fref = module.declare_func_in_func(sum_sq_id, b.func);
        let call = b.ins().call(fref, &[addr]);
        let res = b.inst_results(call)[0];
        b.ins().return_(&[res]);
        b.seal_all_blocks();
        b.finalize(module.target_config());
    }
    module.define_function(main_id, &mut ctx2)?;
    module.finalize_definitions()?;

    let code = module.get_finalized_function(main_id);
    let f: extern "C" fn() -> i64 = unsafe { std::mem::transmute(code) };
    let result = f();
    println!("main() вернул: {}", result);
    println!("ожидалось: 30 (1+4+9+16)");

    Ok(())
}
