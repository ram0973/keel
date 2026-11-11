// Эксперимент: та же функция Keel, но сгенерированная через Cranelift.
//
// Keel:  fn sum_sq(a [int; 4]) int { ... }
// Здесь: та же логика, вручную построенная в SSA-форме CLIF.
//
// Цель — не «сделать компилятор», а измерить реальный объём работы:
// сколько нужно кода, чтобы вместо текста C отдать настоящий IR.

use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::immediates::Imm64;
use cranelift_codegen::ir::stackslot::StackSlotKind;
use cranelift_codegen::ir::types;
use cranelift_codegen::ir::{AbiParam, BlockArg, InstBuilder, MemFlagsData};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{default_libcall_names, Linkage, Module};

fn v(x: cranelift_codegen::ir::Value) -> BlockArg {
    BlockArg::Value(x)
}

/// Строит тело sum_sq: та же логика, что и в Keel, но в SSA-форме CLIF.
fn build_sum_sq(b: &mut FunctionBuilder) {
    let entry = b.create_block();
    b.append_block_params_for_function_params(entry);
    b.seal_block(entry);
    b.switch_to_block(entry);

    let a = b.block_params(entry)[0];
    let total0 = b.ins().iconst(types::I64, 0);
    let i0 = b.ins().iconst(types::I64, 0);

    let header = b.create_block();
    let body = b.create_block();
    let exit = b.create_block();

    // Параметры блока — замена phi-инструкций в Cranelift.
    b.append_block_param(header, types::I64); // total
    b.append_block_param(header, types::I64); // i
    b.append_block_param(body, types::I64); // total
    b.append_block_param(body, types::I64); // i
    b.append_block_param(exit, types::I64); // total

    b.ins().jump(header, &[v(total0), v(i0)]);

    // --- заголовок цикла ---
    b.switch_to_block(header);
    let htotal = b.block_params(header)[0];
    let hi = b.block_params(header)[1];
    let cond = b.ins().icmp_imm_u(IntCC::UnsignedLessThan, hi, Imm64::new(4));
    b.ins()
        .brif(cond, body, &[v(htotal), v(hi)], exit, &[v(htotal)]);

    // --- тело цикла ---
    b.switch_to_block(body);
    let btotal = b.block_params(body)[0];
    let bi = b.block_params(body)[1];
    let off = b.ins().imul_imm(bi, 8);
    let addr = b.ins().iadd(a, off);
    let val = b.ins().load(types::I64, MemFlagsData::trusted(), addr, 0);
    let sq = b.ins().imul(val, val);
    let total2 = b.ins().iadd(btotal, sq);
    let i2 = b.ins().iadd_imm(bi, 1);
    b.ins().jump(header, &[v(total2), v(i2)]);

    // --- выход ---
    b.switch_to_block(exit);
    let etotal = b.block_params(exit)[0];
    b.ins().return_(&[etotal]);

    b.seal_block(header);
    b.seal_block(body);
    b.seal_block(exit);
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // ============ JIT ============
    // ISA выводится сама (из текущей платформы), флаги — списком пар.
    let jit_builder = JITBuilder::with_flags(
        &[("opt_level", "speed")],
        default_libcall_names(),
    )?;
    let mut module = JITModule::new(jit_builder);

    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(types::I64));
    sig.returns.push(AbiParam::new(types::I64));
    let sum_sq_id = module.declare_function("sum_sq", Linkage::Export, &sig)?;

    let mut ctx = module.make_context();
    ctx.func.signature = sig.clone();
    let mut fb_ctx = FunctionBuilderContext::new();
    {
        let mut b = FunctionBuilder::new(&mut ctx.func, &mut fb_ctx);
        build_sum_sq(&mut b);
    }

    println!("=== CLIF для sum_sq ===");
    println!("{}", ctx.func.display());

    module.define_function(sum_sq_id, &mut ctx)?;

    // ============ main: массив в стеке, вызов sum_sq ============
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

        // Массив [1, 2, 3, 4] в стеке.
        let ss = b.create_sized_stack_slot(
            cranelift_codegen::ir::stackslot::StackSlotData::new(
                StackSlotKind::ExplicitSlot,
                32,
                3,
            ),
        );
        for (i, n) in [1i64, 2, 3, 4].iter().enumerate() {
            let c = b.ins().iconst(types::I64, *n);
            b.ins().stack_store(types::I64, c, ss, (i * 8) as i32);
        }

        let addr = b.ins().stack_addr(types::I64, ss, 0);
        let fref = module.declare_func_in_func(sum_sq_id, b.func);
        let call = b.ins().call(fref, &[addr]);
        let res = b.inst_results(call)[0];
        b.ins().return_(&[res]);
        b.seal_all_blocks();
    }
    module.define_function(main_id, &mut ctx2)?;

    module.finalize_definitions()?;

    // ============ AOT: объектный файл через cranelift-object ============
    {
        use cranelift_codegen::ir::stackslot::StackSlotData;
        use cranelift_object::{ObjectBuilder, ObjectModule};

        let t_aot = std::time::Instant::now();
        let mut flag_builder = settings::builder();
        flag_builder.set("opt_level", "speed")?;
        let flags = settings::Flags::new(flag_builder);
        let obj_builder = ObjectBuilder::new(
            cranelift_native::builder()?.finish(flags)?,
            "keel_clif.o".to_string(),
            cranelift_module::default_libcall_names(),
        )?;
        let mut obj_module = ObjectModule::new(obj_builder);
        let mut s = obj_module.make_signature();
        s.params.push(AbiParam::new(types::I64));
        s.returns.push(AbiParam::new(types::I64));
        let f_id = obj_module.declare_function("sum_sq", Linkage::Export, &s)?;
        let mut c = obj_module.make_context();
        c.func.signature = s.clone();
        let mut fbc = FunctionBuilderContext::new();
        {
            let mut b = FunctionBuilder::new(&mut c.func, &mut fbc);
            build_sum_sq(&mut b);
        }
        obj_module.define_function(f_id, &mut c)?;

        let mut m = obj_module.make_signature();
        m.returns.push(AbiParam::new(types::I64));
        let mid = obj_module.declare_function("keel_main", Linkage::Export, &m)?;
        let mut c2 = obj_module.make_context();
        c2.func.signature = m.clone();
        let mut fbc2 = FunctionBuilderContext::new();
        {
            let mut b = FunctionBuilder::new(&mut c2.func, &mut fbc2);
            let entry = b.create_block();
            b.append_block_params_for_function_params(entry);
            b.seal_block(entry);
            b.switch_to_block(entry);
            let ss = b.create_sized_stack_slot(StackSlotData::new(
                StackSlotKind::ExplicitSlot,
                32,
                3,
            ));
            for (i, n) in [1i64, 2, 3, 4].iter().enumerate() {
                let cc = b.ins().iconst(types::I64, *n);
                b.ins().stack_store(types::I64, cc, ss, (i * 8) as i32);
            }
            let addr = b.ins().stack_addr(types::I64, ss, 0);
            let fref = obj_module.declare_func_in_func(f_id, b.func);
            let call = b.ins().call(fref, &[addr]);
            let res = b.inst_results(call)[0];
            b.ins().return_(&[res]);
            b.seal_all_blocks();
        }
        obj_module.define_function(mid, &mut c2)?;

        let buf = obj_module.finish().emit()?;
        std::fs::write("/tmp/keel_clif.o", &buf)?;
        println!(
            "AOT Cranelift: {:?}, объектный файл {} байт",
            t_aot.elapsed(),
            buf.len()
        );
    }

    // Время компиляции: отдельно замеряем, сколько занимает кодогенерация.
    let t_start = std::time::Instant::now();
    let jit_builder2 = JITBuilder::with_flags(
        &[("opt_level", "speed")],
        default_libcall_names(),
    )?;
    let mut module2 = JITModule::new(jit_builder2);
    let mut sig3 = module2.make_signature();
    sig3.params.push(AbiParam::new(types::I64));
    sig3.returns.push(AbiParam::new(types::I64));
    let id3 = module2.declare_function("sum_sq", Linkage::Export, &sig3)?;
    let mut ctx3 = module2.make_context();
    ctx3.func.signature = sig3.clone();
    let mut fbc3 = FunctionBuilderContext::new();
    {
        let mut b = FunctionBuilder::new(&mut ctx3.func, &mut fbc3);
        build_sum_sq(&mut b);
    }
    module2.define_function(id3, &mut ctx3)?;
    module2.finalize_definitions()?;
    println!("время кодогенерации Cranelift: {:?}", t_start.elapsed());

    // Вызываем main через JIT.
    let code = module.get_finalized_function(main_id);
    let f: extern "C" fn() -> i64 = unsafe { std::mem::transmute(code) };
    let result = f();
    println!("main() вернул: {}", result);
    println!("ожидалось: 30 (1+4+9+16)");

    Ok(())
}
