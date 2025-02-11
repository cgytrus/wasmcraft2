use std::collections::{HashSet, HashMap};

use crate::{lir::{Register, DoubleRegister}};

use super::{SsaFunction, SsaVar, liveness::{NoopLivenessInfo, LivenessInfo, FullLivenessInfo}, TypedSsaVar, BlockId};

pub trait RegAlloc {
	//fn analyze(ssa_func: &SsaFunction) -> Self;

	fn get(&self, val: SsaVar) -> Register;

	fn get_double(&self, val: SsaVar) -> DoubleRegister;

	fn get_const(&mut self, val: i32) -> Register;

	fn get_double_const(&mut self, val: i64) -> DoubleRegister;

	fn get_temp(&mut self) -> Register;

	fn get_temp_double(&mut self) -> DoubleRegister;

	fn const_pool(&self) -> &HashSet<i32>;
}

pub struct NoopRegAlloc {
	pub const_pool: HashSet<i32>,
	func: u32,
	temp: u32,
}

impl NoopRegAlloc {
	pub fn analyze(func: &SsaFunction) -> Self {
		NoopRegAlloc { const_pool: HashSet::new(), func: func.iter().next().unwrap().0.func as u32, temp: 1000 }
	}
}

impl RegAlloc for NoopRegAlloc {
	fn get(&self, val: SsaVar) -> Register {
		Register::work_lo(self.func, val.0)
	}

	fn get_double(&self, val: SsaVar) -> DoubleRegister {
		DoubleRegister::Work(self.func, val.0)
	}

	fn get_const(&mut self, val: i32) -> Register {
		self.const_pool.insert(val);
		Register::const_val(val)
	}

	fn get_double_const(&mut self, val: i64) -> DoubleRegister {
		self.const_pool.insert(val as i32);
		self.const_pool.insert((val >> 32) as i32);
		DoubleRegister::const_val(val)
	}

	fn get_temp(&mut self) -> Register {
		let reg = Register::temp_lo(self.temp);
		self.temp += 1;
		reg
	}

	fn get_temp_double(&mut self) -> DoubleRegister {
		let reg = DoubleRegister::temp(self.temp);
		self.temp += 1;
		reg
	}

	fn const_pool(&self) -> &HashSet<i32> {
		&self.const_pool
	}
}

pub struct FullRegAlloc {
	pub const_pool: HashSet<i32>,
	pub map: HashMap<SsaVar, u32>,
	func: u32,
	temp: u32,
}

mod register_set {
	use std::collections::{HashMap, BTreeSet};

	use wasmparser::ValType;

	use crate::ssa::{liveness::LiveRange, SsaInstr};

	use super::*;

	#[derive(Debug)]
	pub struct InterfGraph(HashMap<TypedSsaVar, Vec<TypedSsaVar>>);

	impl InterfGraph {
		pub fn new(func: &SsaFunction) -> Self {
			let mut result = InterfGraph(HashMap::new());

			for (_, block) in func.iter() {
				for instr in block.body.iter() {
					match instr {
						SsaInstr::Add(dst, lhs, rhs) if dst.ty() == ValType::I64 => {
							if let Some(l) = lhs.get_var() {
								result.add_edge(*dst, l)
							}
							if let Some(r) = rhs.get_var() {
								result.add_edge(*dst, r);
							}
							if let (Some(l), Some(r)) = (lhs.get_var(), rhs.get_var()) {
								result.add_edge(l, r);
							}
						}
						SsaInstr::Ctz(dst, src) if dst.ty() == ValType::I64 => {
							result.add_edge(*dst, *src);
						}
						SsaInstr::GeU(dst, lhs, rhs) |
						SsaInstr::GtU(dst, lhs, rhs) |
						SsaInstr::LeU(dst, lhs, rhs) |
						SsaInstr::LtU(dst, lhs, rhs) => {
							if let Some(l) = lhs.get_var() {
								result.add_edge(*dst, l);
							}
							if let Some(r) = rhs.get_var() {
								result.add_edge(*dst, r);
							}
						}
						SsaInstr::GeS(dst, lhs, rhs) |
						SsaInstr::GtS(dst, lhs, rhs) |
						SsaInstr::LeS(dst, lhs, rhs) |
						SsaInstr::LtS(dst, lhs, rhs) if dst.ty() == ValType::I64 => {
							if let Some(l) = lhs.get_var() {
								result.add_edge(*dst, l);
							}
							if let Some(r) = rhs.get_var() {
								result.add_edge(*dst, r);
							}
						}
						SsaInstr::RemU(dst, lhs, rhs) if dst.ty() == ValType::I32 => {
							result.add_edge(*dst, *lhs);
							if let Some(r) = rhs.get_var() {
								result.add_edge(*dst, r);
							}
						}
						_ => {}
					}
				}
			}

			result
		}

		pub fn add_edge(&mut self, a: TypedSsaVar, b: TypedSsaVar) {
			self.add_dir_edge(a, b);
			self.add_dir_edge(b, a);
		}

		fn add_dir_edge(&mut self, a: TypedSsaVar, b: TypedSsaVar) {
			let others = self.0.entry(a).or_insert_with(Vec::new);
			if !others.contains(&b) {
				others.push(b);
			}
		}

		pub fn interferes(&self, lhs: &MergedRegister, rhs: &MergedRegister) -> bool {
			for lhs_reg in lhs.members.iter() {
				if let Some(interf_regs) = self.0.get(lhs_reg) {
					for rhs_reg in interf_regs.iter() {
						if rhs.members.contains(rhs_reg) {
							return true;
						}
					}
				}
			}

			false
		}
	}

	#[derive(Debug)]
	pub struct MergedRegister {
		pub members: BTreeSet<TypedSsaVar>,
		pub live_range: LiveRange,
	}

	#[derive(Debug)]
	pub struct RegisterSet(Vec<MergedRegister>);

	impl RegisterSet {
		pub fn new(func: &SsaFunction) -> Self {
			let all_vars = NoopLivenessInfo::analyze(func).vars;

			let live_info = FullLivenessInfo::analyze(func);

			//crate::ssa::liveness::print_liveness_info(&live_info, func);

			//println!("Need to create {} registers in set", all_vars.len());

			let regs = all_vars.into_iter().map(|var| {
				let members = Some(var).into_iter().collect();
				let live_range = live_info.live_range(var);
				MergedRegister { members, live_range }
			}).collect();

			RegisterSet(regs)
		}

		pub fn len(&self) -> usize {
			self.0.len()
		}

		pub fn to_map(&self) -> HashMap<SsaVar, u32> {
			let mut result = HashMap::new();
			for merged_reg in self.0.iter() {
				let rep = merged_reg.members.iter().next().unwrap();
				for reg in merged_reg.members.iter().copied() {
					result.insert(reg.into_untyped(), rep.0);
				}
			}
			result
		}

		pub fn get(&self, elem: TypedSsaVar) -> &MergedRegister {
			self.0.iter().find(|reg| reg.members.contains(&elem)).unwrap()
		}

		pub fn merge(&mut self, var1: TypedSsaVar, var2: TypedSsaVar) {
			let r1 = self.get_idx(var1);
			let mut merged1 = self.0.remove(r1);

			let r2 = self.get_idx(var2);
			let merged2 = self.0.remove(r2);

			merged1.members.extend(merged2.members);
			merged1.live_range.merge(merged2.live_range);

			self.0.push(merged1);
		}

		pub fn get_idx(&self, var1: TypedSsaVar) -> usize {
			self.0.iter().enumerate().find(|(_, reg)| reg.members.contains(&var1)).unwrap().0
		}

		pub fn get_idx2(&self, var1: SsaVar) -> usize {
			self.0.iter().enumerate().find(|(_, reg)| reg.members.iter().any(|r| r.0 == var1.0)).unwrap().0
		}

		pub fn get_rep(&self, var1: SsaVar) -> TypedSsaVar {
			let idx = self.get_idx2(var1);
			*self.0[idx].members.iter().next().unwrap()
		}
	}
}

use register_set::*;

fn try_merge_term(sets: &mut RegisterSet, block_id: BlockId, dst: &TypedSsaVar, src: &TypedSsaVar, func: &SsaFunction, interf_graph: &InterfGraph) {
	if sets.get_idx(*dst) == sets.get_idx(*src) {
		return;
	}

	let dst_set = sets.get(*dst);
	let src_set = sets.get(*src);

	if dst_set.live_range.is_empty() {
		panic!("merging to dead destination {:?} {:?}", block_id, dst);
	}

	if src_set.live_range.is_empty() {
		panic!("merging from dead source")
	}

	if interf_graph.interferes(dst_set, src_set) {
		return;
	}

	let block = func.get(block_id);

	for succ_block in block.term.successors() {
		if dst_set.live_range.0.get(succ_block).unwrap().live_in {
			// This variable isn't actually defined by the terminator, so don't try to coalesce it.
			return
		}

		/*if !dst_set.live_range.0.get(&succ_block).unwrap().body_contains(0) {
			// This variable isn't actually alive.
			panic!("variable is not alive {:?} {:?}", dst, block_id);
		}*/
	}

	let overlap = dst_set.live_range.overlap(&src_set.live_range);

	//println!("Registers {:?} {:?}", dst, src);
	//print_live_ranges(&[dst_set.live_range.clone(), src_set.live_range.clone()], func);

	if overlap.is_empty() {
		/*let v74 = TypedSsaVar(20, wasmparser::ValType::I32);
		let v76 = TypedSsaVar(45, wasmparser::ValType::I32);
		let ok1 = dst_set.members.contains(&v74) && src_set.members.contains(&v76);
		let ok2 = dst_set.members.contains(&v76) && src_set.members.contains(&v74);
		if ok1 || ok2 {
			panic!();
		}*/

		sets.merge(*dst, *src);
	}
}

fn try_merge(sets: &mut RegisterSet, block_id: BlockId, instr_idx: usize, dst: &TypedSsaVar, src: &TypedSsaVar, interf_graph: &InterfGraph) {
	if sets.get_idx(*dst) == sets.get_idx(*src) {
		return;
	}

	let dst_set = sets.get(*dst);
	let src_set = sets.get(*src);

	if interf_graph.interferes(dst_set, src_set) {
		return;
	}

	if dst_set.live_range.is_empty() {
		//panic!("merging to dead destination {:?} {:?}", block_id, dst);
		println!("merging to dead destination {:?} {:?}", block_id, dst);
		return;
	}

	if src_set.live_range.is_empty() {
		//panic!("merging from dead source");
		println!("merging from dead source");
		return;
	}

	let overlap = dst_set.live_range.overlap(&src_set.live_range);

	if overlap.is_empty() {
		let dst_block_live_range = dst_set.live_range.0.get(block_id).unwrap();
		if !dst_block_live_range.body.iter().any(|r| r.contains(&(instr_idx + 1))) {
			panic!();
		}

		sets.merge(*dst, *src);
	}
}

impl FullRegAlloc {
	pub fn analyze(func: &SsaFunction) -> Self {
		//println!("Starting regalloc for {}", func.func_id());

		let interf_graph = InterfGraph::new(func);

		let mut sets = RegisterSet::new(func);

		//println!("Register sets created for {}", func.func_id());

		for (block_id, block) in func.iter() {
			for (instr_idx, instr) in block.body.iter().enumerate() {
				for (dst, src) in instr.coalescable_vars() {
					let uses = instr.uses();
					let defs = instr.defs();
					assert!(uses.contains(src));
					assert!(defs.contains(dst));

					try_merge(&mut sets, block_id, instr_idx, dst, src, &interf_graph);
				}
			}

			for (dst, src) in func.coalescable_term_vars(block_id) {
				try_merge_term(&mut sets, block_id, &dst, &src, func, &interf_graph);
			}
		}

		println!("Coalesced func {} into {} registers", func.func_id(), sets.len());

		FullRegAlloc { const_pool: HashSet::new(), map: sets.to_map(), func: func.func_id(), temp: 1000 }
	}
}

impl RegAlloc for FullRegAlloc {
	fn get(&self, val: SsaVar) -> Register {
		let rep = *self.map.get(&val).unwrap();
		Register::work_lo(self.func, rep)
	}

	fn get_double(&self, val: SsaVar) -> DoubleRegister {
		let rep = *self.map.get(&val).unwrap();
		DoubleRegister::Work(self.func, rep)
	}

	fn get_const(&mut self, val: i32) -> Register {
		self.const_pool.insert(val);
		Register::const_val(val)
	}

	fn get_double_const(&mut self, val: i64) -> DoubleRegister {
		self.const_pool.insert(val as i32);
		self.const_pool.insert((val >> 32) as i32);
		DoubleRegister::const_val(val)
	}

	fn get_temp(&mut self) -> Register {
		let reg = Register::temp_lo(self.temp);
		self.temp += 1;
		reg
	}

	fn get_temp_double(&mut self) -> DoubleRegister {
		let reg = DoubleRegister::temp(self.temp);
		self.temp += 1;
		reg
	}

	fn const_pool(&self) -> &HashSet<i32> {
		&self.const_pool
	}
}

#[cfg(test)]
mod test {
	use wasmparser::ValType;

	use crate::ssa::{SsaBasicBlock, TypedSsaVar, SsaInstr, SsaTerminator, JumpTarget, BlockId, SsaFunction};

	use super::{FullRegAlloc, RegAlloc};

	/*#[test]
	fn reg_across_jump() {
		let r0 = TypedSsaVar(0, ValType::I32);
		let r1 = TypedSsaVar(1, ValType::I32);

		let b0 = SsaBasicBlock {
			// TODO: ???
			params: Vec::new(),
			body: vec![
				SsaInstr::I32Set(r0, 123),
			],
			term: SsaTerminator::Jump(JumpTarget {
				label: BlockId { func: 0, block: 1 },
				params: vec![r0],
			})
		};
	}*/

	#[test]
	#[ignore]
	fn no_coalesce_dead_register() {
		// TODO: Also use the palette test for this.

		let r0 = TypedSsaVar(0, ValType::I32);
		let r1 = TypedSsaVar(1, ValType::I32);

		let r2 = TypedSsaVar(2, ValType::I32);
		let r3 = TypedSsaVar(3, ValType::I32);

		let r4 = TypedSsaVar(4, ValType::I32);

		let b1 = SsaBasicBlock {
			params: Vec::new(),
			body: vec![
				SsaInstr::I32Set(r0, 42),
				SsaInstr::I32Set(r1, 17),
			],
			term: SsaTerminator::Jump(JumpTarget {
				label: BlockId { func: 0, block: 0 },
				params: vec![r0, r1],
			})
		};

		let b0 = SsaBasicBlock {
			params: vec![r2, r3],
			body: vec![
				SsaInstr::Assign(r3, r2.into()), // This convinces the regalloc to try coalescing r3 and r2, which triggers the bug.
				SsaInstr::Or(r4, r3, r3.into()), // Use this instruction so that it doesn't try to coalesce r4 and r3
			],
			term: SsaTerminator::Return(vec![r2]),
		};

		let func = SsaFunction::new(
			vec![
				// It's also order-dependent, too.
				(BlockId { func: 0, block: 0 }, b0),
				(BlockId { func: 0, block: 1 }, b1),
			],
			Box::new([]),
			Box::new([ValType::I32]),
		);

		let reg_alloc = FullRegAlloc::analyze(&func);

		let reg0 = reg_alloc.get(r0.into_untyped());
		let reg1 = reg_alloc.get(r1.into_untyped());
		let reg2 = reg_alloc.get(r2.into_untyped());
		let reg3 = reg_alloc.get(r3.into_untyped());

		assert_eq!(reg0, reg2);
		assert_eq!(reg1, reg3);
		assert_ne!(reg0, reg1);

		panic!("{:?}", reg_alloc.map);
	}

	#[test]
	#[ignore]
	fn coalesce_jump_param() {
		let r0 = TypedSsaVar(0, ValType::I32);
		let r1 = TypedSsaVar(1, ValType::I32);

		let b0 = SsaBasicBlock {
			params: Vec::new(),
			body: vec![
				SsaInstr::I32Set(r0, 123)
			],
			term: SsaTerminator::Jump(JumpTarget {
				label: BlockId { func: 0, block: 1},
				params: vec![r0]
			})
		};

		let b1 = SsaBasicBlock {
			params: vec![r1],
			body: Vec::new(),
			term: SsaTerminator::Return(vec![r1]),
		};

		let func = SsaFunction::new(
			vec![
				(BlockId { func: 0, block: 1 }, b1),
				(BlockId { func: 0, block: 0 }, b0),
			],
			Box::new([]),
			Box::new([ValType::I32]),
		);

		let reg_alloc = FullRegAlloc::analyze(&func);

		println!("{:?}", reg_alloc.map);
		panic!();

		/*let liveness = FullLivenessInfo::analyze(&func);

		let r0_range = liveness.live_range(r0);
		let r1_range = liveness.live_range(r1);

		print_liveness_info(&liveness, &func);
		println!();

		println!("{:?}\n", r0_range);
		println!("{:?}\n", r1_range);
		println!("{:?}\n", overlap);
		panic!();*/
	}
}
