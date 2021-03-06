use parity_wasm::elements::*;

mod classifications;
mod errors;

use crate::classifications::*;
use crate::errors::*;
use self::Filter::*;
use std::mem::discriminant;

/*
 * TODO:
 * 1. Proper error propagating
 * 2. Documentation
 * 3. Good tests with expected failures
 */

/// Type alias representing the values popped from the stack by a given operation.
/// Mostly just for readability
type Pop = Vec<ValueType>;
/// Type alias representing the values pushed onto the stack by a given operation.
/// Mostly just for readability
type Push = Vec<ValueType>;

/// The "signature" of a type, meaning the values pushed and popped from the stack by the operation.
struct Signature {
	pop: Pop,
	push: Push
}

/// An enum representing which instructions should be validated
pub enum Filter {
	NumericInstructions,
	NoFilter
}

/// Basic struct for validating modules
pub struct ModuleValidator<'a> {
	module: &'a Module,
	filter: Filter,
	stack: Vec<ValueType>
}

impl<'a> ModuleValidator<'a> {

	/// Convenience method for creating a new validator
	pub fn new(module: &'a Module, filter: Filter) -> Self {
		ModuleValidator{ module, filter, stack: vec![] }
	}

	/// Handler method that loops over functions and delegates validation to `check_instructions`
	pub fn validate(&mut self) -> Result<bool, InstructionError> {
		match self.module.code_section() {
			Some(functions) => {
				for (index, function) in functions.bodies().iter().enumerate() {
					let is_function_valid: bool = self.check_instructions(function, index)?;
					if !is_function_valid {
						return Ok(false)
					}
				}
				Ok(true)
			},
			None => Ok(true),
		}
	}

	/// A method used to determine what the classification of each instruction, and execute the correct method on it
	fn check_instructions(&mut self, body: &FuncBody, index: usize) -> Result<bool, InstructionError> {
		for instruction in body.code().elements() {
			if contains(instruction, &GET_INST) && !self.push_global_or_local(instruction, body, index)? {
					return Ok(false)
			}
			match self.filter {
				NumericInstructions => {
					let signature = get_instruction_signature(instruction);
					// if the instruction does not have a signature we are interested in, we continue
					if signature.is_some() && !self.validate_instruction(&signature.unwrap(), instruction)? {
						return Ok(false)
					}					
				}
				NoFilter => () // TODO: do this
			};
		}
		Ok(true)
	}

	/// Evaluates a signature and determines if the stack can support the instruction in it's current state
	fn validate_instruction(&mut self, signature: &Signature, instruction: &Instruction) -> Result<bool, InstructionError> {
		for signature_value in &signature.pop {
			let value = self.stack.pop();
			match value {
				Some(stack_value) => {
					if stack_value != *signature_value {
						return Err(InstructionError::InvalidOperation(instruction.clone()))
					}
				}
				None => return Err(InstructionError::InvalidOperation(instruction.clone())) // Instructions are small, so clone

			}
		}
		self.stack.extend(&signature.push);

		Ok(true)
	}

	/// A method used to load global or local variable types onto the stack
	fn push_global_or_local(&mut self, instruction: &Instruction, body: &FuncBody, index: usize) -> Result<bool, InstructionError> {

		// These next couple lines are just to get the parameters of the function we're dealing with.
		// We need the parameters because they can be loaded like local variables but they're not in the locals vec

		// type_ref is the index of the FunctionType in types_section
		let type_ref = self.module.function_section().unwrap().entries()[index].type_ref();
		let type_variant = &self.module.type_section().unwrap().types()[type_ref as usize];

		let mut locals = body.locals().to_vec();
		match type_variant {
			Type::Function(ftype) => {
				locals.extend(ftype.params().iter().map(|f| Local::new(0, *f)));
			}
		}

		match instruction {
			Instruction::GetGlobal(local) => {
				match locals.get(*local as usize) {
					Some(variable) => {
						self.stack.push(variable.value_type());
						Ok(true)
					},
					None => { Err(InstructionError::GlobalNotFound) },
				}
			},
			Instruction::GetLocal(local) => {
				match locals.get(*local as usize) {
					Some(variable) => {
						self.stack.push(variable.value_type());
						Ok(true)
					},
					None => { Err(InstructionError::LocalNotFound) },
				}
			},
			_ => { Err(InstructionError::UnmatchedInstruction) },
		}
	}
}

/// Checks the **discriminant** of an instruction against the **discriminants** of a container,
/// to determine if the given instruction is in the container while ignoring values.
fn contains(instruction: &Instruction, container: &[Instruction]) -> bool {
	container.iter().any(|f| discriminant(f) == discriminant(instruction))
}

/// Given an instruction, determine it's signature based on what classification it is in.
fn get_instruction_signature(instruction: &Instruction) -> Option<Signature> {
	// returns some signature if there is a type we are interested in
	// returns None otherwise
	if contains(instruction, &I32_BINOP) {
		Some(Signature{ pop: [ValueType::I32; 2].to_vec(), push: [ValueType::I32; 1].to_vec() })
	} else if contains(instruction, &I64_BINOP) {
		Some(Signature{ pop: [ValueType::I64; 2].to_vec(), push: [ValueType::I64; 1].to_vec() })
	} else if contains(instruction, &F32_BINOP) {
		Some(Signature{ pop: [ValueType::F32; 2].to_vec(), push: [ValueType::F32; 1].to_vec() })
	} else if contains(instruction, &F64_BINOP) {
		Some(Signature{ pop: [ValueType::F64; 2].to_vec(), push: [ValueType::F64; 1].to_vec() })
	} else if contains(instruction, &CONST_INST) {
		get_const_signature(instruction)
	} else {
		None
	}
}

/// Determines the signature of a const instruction, which are slightly different from regular instructions	
fn get_const_signature(instruction: &Instruction) -> Option<Signature> {
	let inst_type = &format!("{:?}", instruction)[..3];

	match inst_type {
		"I32" => Some(Signature{ pop: [].to_vec(), push: [ValueType::I32; 1].to_vec() }),
		"I64" => Some(Signature{ pop: [].to_vec(), push: [ValueType::I64; 1].to_vec() }),
		"F32" => Some(Signature{ pop: [].to_vec(), push: [ValueType::F32; 1].to_vec() }),
		"F64" => Some(Signature{ pop: [].to_vec(), push: [ValueType::F64; 1].to_vec() }),
		_ => None
	}
}


#[cfg(test)]
mod tests {
	use super::*;
	use parity_wasm::elements::deserialize_buffer;
	use parity_wasm::deserialize_file;

	#[test]
	fn simple_instructions_binary() {
		// WAST:
		// (module
		//   (type $t0 (func (param i32 i32) (result i32)))
		//   (func $f0 (type $t0) (param $p0 i32) (param $p1 i32) (result i32)
		//     (i32.add
		//       (get_local $p0)
		//       (get_local $p1))))
		let wasm: Vec<u8> = vec![
			0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x07, 0x01, 0x60, 0x02, 0x7f, 0x7f, 0x01,
			0x7f, 0x03, 0x02, 0x01, 0x00, 0x0a, 0x09, 0x01, 0x07, 0x00, 0x20, 0x00, 0x20, 0x01, 0x6a, 0x0b,
			0x00, 0x14, 0x04, 0x6e, 0x61, 0x6d, 0x65, 0x02, 0x0d, 0x01, 0x00, 0x02, 0x00, 0x03, 0x6c, 0x68,
			0x73, 0x01, 0x03, 0x72, 0x68, 0x73
		];

		let module = deserialize_buffer::<Module>(&wasm).unwrap();

		let mut validator = ModuleValidator::new(&module, NumericInstructions);
		let is_valid = validator.validate().unwrap();
		assert!(is_valid)
	}

	#[test]
	fn const_instructions_binary() {
		// WAST:
		// (module
		//   (table 0 anyfunc)
		//   (memory $0 1)
		//   (export "memory" (memory $0))
		//   (export "_Z9i32_constv" (func $_Z9i32_constv))
		//   (export "_Z9i64_constv" (func $_Z9i64_constv))
		//   (export "_Z9f32_constv" (func $_Z9f32_constv))
		//   (export "_Z9f64_constv" (func $_Z9f64_constv))
		//   (func $_Z9i32_constv (; 0 ;) (result i32)
		//     (i32.const 2147483647)
		//   )
		//   (func $_Z9i64_constv (; 1 ;) (result i64)
		//     (i64.const 9223372036854775807)
		//   )
		//   (func $_Z9f32_constv (; 2 ;) (result f32)
		//     (f32.const 3402823466385288598117041e14)
		//   )
		//   (func $_Z9f64_constv (; 3 ;) (result f64)
		//     (f64.const 1.8e+307)
		//   )
		// )
		let wasm: Vec<u8> = vec![
			0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x91, 0x80, 0x80, 0x80, 0x00, 0x04, 0x60,
			0x00, 0x01, 0x7f, 0x60, 0x00, 0x01, 0x7e, 0x60, 0x00, 0x01, 0x7d, 0x60, 0x00, 0x01, 0x7c, 0x03,
			0x85, 0x80, 0x80, 0x80, 0x00, 0x04, 0x00, 0x01, 0x02, 0x03, 0x04, 0x84, 0x80, 0x80, 0x80, 0x00,
			0x01, 0x70, 0x00, 0x00, 0x05, 0x83, 0x80, 0x80, 0x80, 0x00, 0x01, 0x00, 0x01, 0x06, 0x81, 0x80,
			0x80, 0x80, 0x00, 0x00, 0x07, 0xca, 0x80, 0x80, 0x80, 0x00, 0x05, 0x06, 0x6d, 0x65, 0x6d, 0x6f,
			0x72, 0x79, 0x02, 0x00, 0x0d, 0x5f, 0x5a, 0x39, 0x69, 0x33, 0x32, 0x5f, 0x63, 0x6f, 0x6e, 0x73,
			0x74, 0x76, 0x00, 0x00, 0x0d, 0x5f, 0x5a, 0x39, 0x69, 0x36, 0x34, 0x5f, 0x63, 0x6f, 0x6e, 0x73,
			0x74, 0x76, 0x00, 0x01, 0x0d, 0x5f, 0x5a, 0x39, 0x66, 0x33, 0x32, 0x5f, 0x63, 0x6f, 0x6e, 0x73,
			0x74, 0x76, 0x00, 0x02, 0x0d, 0x5f, 0x5a, 0x39, 0x66, 0x36, 0x34, 0x5f, 0x63, 0x6f, 0x6e, 0x73,
			0x74, 0x76, 0x00, 0x03, 0x0a, 0xbc, 0x80, 0x80, 0x80, 0x00, 0x04, 0x88, 0x80, 0x80, 0x80, 0x00,
			0x00, 0x41, 0xff, 0xff, 0xff, 0xff, 0x07, 0x0b, 0x8d, 0x80, 0x80, 0x80, 0x00, 0x00, 0x42, 0xff,
			0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00, 0x0b, 0x87, 0x80, 0x80, 0x80, 0x00, 0x00,
			0x43, 0xff, 0xff, 0x7f, 0x7f, 0x0b, 0x8b, 0x80, 0x80, 0x80, 0x00, 0x00, 0x44, 0x2e, 0x02, 0x68,
			0x83, 0x02, 0xa2, 0xb9, 0x7f, 0x0b
		];

		let module = deserialize_buffer::<Module>(&wasm).unwrap();

		let mut validator = ModuleValidator::new(&module, NumericInstructions);
		let is_valid = validator.validate().unwrap();
		assert!(is_valid)
	}

	#[test]
	#[should_panic]
	fn unmatched_type_failure_binary() {
		// Binary incorrectly tries to add an f64 with i32 to return an i32
		// This should be failed by the validator
		// WAST:
		// (module
		//   (func $addTwo (param f64 i32) (result i32)
		//     get_local 0
		//     get_local 1
		//     i32.add)
		//   (export "addTwo" (func $addTwo)))
		let wasm: Vec<u8> = vec![
			0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x07, 0x01, 0x60, 0x02, 0x7c, 0x7f, 0x01,
			0x7f, 0x03, 0x02, 0x01, 0x00, 0x07, 0x0a, 0x01, 0x06, 0x61, 0x64, 0x64, 0x54, 0x77, 0x6f, 0x00,
			0x00, 0x0a, 0x09, 0x01, 0x07, 0x00, 0x20, 0x00, 0x20, 0x01, 0x6a, 0x0b, 0x00, 0x19, 0x04, 0x6e,
			0x61, 0x6d, 0x65, 0x01, 0x09, 0x01, 0x00, 0x06, 0x61, 0x64, 0x64, 0x54, 0x77, 0x6f, 0x02, 0x07,
			0x01, 0x00, 0x02, 0x00, 0x00, 0x01, 0x00
		];

		let module = deserialize_buffer::<Module>(&wasm).unwrap();

		let mut validator = ModuleValidator::new(&module, NumericInstructions);
		validator.validate().unwrap();
	}

	#[test]
	fn simple_wasm_from_file() {
		// WAST:
		// (module
		//   (func $addTwo (param f64 i32) (result i32)
		//     get_local 0
		//     get_local 1
		//     i32.add)
		//   (export "addTwo" (func $addTwo)))
		let module = deserialize_file("./src/wasm_binaries/test.wasm").unwrap();
		let mut validator = ModuleValidator::new(&module, NumericInstructions);
		let is_valid = validator.validate().unwrap();
		assert!(is_valid)
	}

	#[test]
	fn complex_instruction_binary() {
		// WAST:
		// (module
		//   (type $t0 (func (param i32 i32) (result i32)))
		//   (func $_Z4multii (export "_Z4multii") (type $t0) (param $p0 i32) (param $p1 i32) (result i32)
		//     (i32.mul
		//       (get_local $p1)
		//       (get_local $p0)))
		//   (func $_Z3addii (export "_Z3addii") (type $t0) (param $p0 i32) (param $p1 i32) (result i32)
		//     (i32.add
		//       (get_local $p1)
		//       (get_local $p0)))
		//   (func $_Z6divideii (export "_Z6divideii") (type $t0) (param $p0 i32) (param $p1 i32) (result i32)
		//     (i32.div_s
		//       (get_local $p0)
		//       (get_local $p1)))
		//   (table $T0 0 anyfunc)
		//   (memory $memory (export "memory") 1))

		let wasm: Vec<u8> = vec![
			0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x07, 0x01, 0x60, 0x02, 0x7f, 0x7f, 0x01,
			0x7f, 0x03, 0x04, 0x03, 0x00, 0x00, 0x00, 0x04, 0x04, 0x01, 0x70, 0x00, 0x00, 0x05, 0x03, 0x01,
			0x00, 0x01, 0x07, 0x2f, 0x04, 0x09, 0x5f, 0x5a, 0x34, 0x6d, 0x75, 0x6c, 0x74, 0x69, 0x69, 0x00,
			0x00, 0x08, 0x5f, 0x5a, 0x33, 0x61, 0x64, 0x64, 0x69, 0x69, 0x00, 0x01, 0x0b, 0x5f, 0x5a, 0x36,
			0x64, 0x69, 0x76, 0x69, 0x64, 0x65, 0x69, 0x69, 0x00, 0x02, 0x06, 0x6d, 0x65, 0x6d, 0x6f, 0x72,
			0x79, 0x02, 0x00, 0x0a, 0x19, 0x03, 0x07, 0x00, 0x20, 0x01, 0x20, 0x00, 0x6c, 0x0b, 0x07, 0x00,
			0x20, 0x01, 0x20, 0x00, 0x6a, 0x0b, 0x07, 0x00, 0x20, 0x00, 0x20, 0x01, 0x6d, 0x0b, 0x00, 0x4b,
			0x04, 0x6e, 0x61, 0x6d, 0x65, 0x01, 0x23, 0x03, 0x00, 0x09, 0x5f, 0x5a, 0x34, 0x6d, 0x75, 0x6c,
			0x74, 0x69, 0x69, 0x01, 0x08, 0x5f, 0x5a, 0x33, 0x61, 0x64, 0x64, 0x69, 0x69, 0x02, 0x0b, 0x5f,
			0x5a, 0x36, 0x64, 0x69, 0x76, 0x69, 0x64, 0x65, 0x69, 0x69, 0x02, 0x1f, 0x03, 0x00, 0x02, 0x00,
			0x02, 0x70, 0x30, 0x01, 0x02, 0x70, 0x31, 0x01, 0x02, 0x00, 0x02, 0x70, 0x30, 0x01, 0x02, 0x70,
			0x31, 0x02, 0x02, 0x00, 0x02, 0x70, 0x30, 0x01, 0x02, 0x70, 0x31
		];

		let module = deserialize_buffer::<Module>(&wasm).unwrap();

		let mut validator = ModuleValidator::new(&module, NumericInstructions);
		let is_valid = validator.validate().unwrap();
		assert!(is_valid)
	}
}