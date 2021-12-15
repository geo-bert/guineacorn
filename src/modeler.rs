use anyhow::{Context, Result};
use byteorder::{ByteOrder, LittleEndian};
use log::trace;
use monster::engine::system::SyscallId;
use riscu::{decode, types::*, Instruction, Program, Register};
use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::LinkedList;
use std::hash::{Hash, Hasher};
use std::mem::size_of;
use std::rc::Rc;

//
// Public Interface
//

pub mod optimize;
pub mod unroller;

pub type Nid = u64;
pub type NodeRef = Rc<RefCell<Node>>;

#[derive(Debug)]
pub enum Node {
    Const {
        nid: Nid,
        sort: NodeType,
        imm: u64,
    },
    Read {
        nid: Nid,
        memory: NodeRef,
        address: NodeRef,
    },
    Write {
        nid: Nid,
        memory: NodeRef,
        address: NodeRef,
        value: NodeRef,
    },
    Add {
        nid: Nid,
        left: NodeRef,
        right: NodeRef,
    },
    Sub {
        nid: Nid,
        left: NodeRef,
        right: NodeRef,
    },
    Rem {
        nid: Nid,
        left: NodeRef,
        right: NodeRef,
    },
    Ite {
        nid: Nid,
        sort: NodeType,
        cond: NodeRef,
        left: NodeRef,
        right: NodeRef,
    },
    Eq {
        nid: Nid,
        left: NodeRef,
        right: NodeRef,
    },
    And {
        nid: Nid,
        left: NodeRef,
        right: NodeRef,
    },
    Not {
        nid: Nid,
        value: NodeRef,
    },
    State {
        nid: Nid,
        sort: NodeType,
        init: Option<NodeRef>,
        name: Option<String>,
    },
    Next {
        nid: Nid,
        sort: NodeType,
        state: NodeRef,
        next: NodeRef,
        name: Option<String>,
    },
    Bad {
        nid: Nid,
        cond: NodeRef,
        name: Option<String>,
    },
    Comment(String),
}

#[derive(Clone, Debug)]
pub enum NodeType {
    Bit,
    Word,
    Memory,
}

#[derive(Debug)]
pub struct Model {
    // TODO: Switch from `LinkedList` to `Vec` here.
    pub lines: LinkedList<NodeRef>,
    pub sequentials: Vec<NodeRef>,
    pub bad_states: Vec<NodeRef>,
}

#[derive(Debug)]
pub struct HashableNodeRef {
    value: NodeRef,
}

pub fn generate_model(program: &Program) -> Result<Model> {
    trace!("Program: {:?}", program);
    let mut builder = ModelBuilder::new();
    builder.generate_model(program)?;
    Ok(builder.finalize())
}

#[rustfmt::skip]
pub fn print_model(model: &Model) {
    trace!("Model: {:?}", model);
    println!("; cksystemsgroup.github.io/monster\n");
    println!("1 sort bitvec 1 ; Boolean");
    println!("2 sort bitvec 64 ; 64-bit machine word");
    println!("3 sort array 2 2 ; 64-bit physical memory");
    for node in model.lines.iter() {
        match &*node.borrow() {
            Node::Const { nid, sort, imm } =>
                println!("{} constd {} {}", nid, get_sort(sort), imm),
            Node::Read { nid, memory, address } =>
                println!("{} read 2 {} {}", nid, get_nid(memory), get_nid(address)),
            Node::Write { nid, memory, address, value } =>
                println!("{} write 3 {} {} {}", nid, get_nid(memory), get_nid(address), get_nid(value)),
            Node::Add { nid, left, right } =>
                println!("{} add 2 {} {}", nid, get_nid(left), get_nid(right)),
            Node::Sub { nid, left, right } =>
                println!("{} sub 2 {} {}", nid, get_nid(left), get_nid(right)),
            Node::Rem { nid, left, right } =>
                println!("{} urem 2 {} {}", nid, get_nid(left), get_nid(right)),
            Node::Ite { nid, sort, cond, left, right } =>
                println!("{} ite {} {} {} {}", nid, get_sort(sort), get_nid(cond), get_nid(left), get_nid(right)),
            Node::Eq { nid, left, right } =>
                println!("{} eq 1 {} {}", nid, get_nid(left), get_nid(right)),
            Node::And { nid, left, right } =>
                println!("{} and 1 {} {}", nid, get_nid(left), get_nid(right)),
            Node::Not { nid, value } =>
                println!("{} not 1 {}", nid, get_nid(value)),
            Node::State { nid, sort, init, name } => {
                println!("{} state {} {}", nid, get_sort(sort), name.as_ref().map_or("?", |s| &*s));
                if let Some(value) = init {
                    println!("{} init {} {} {}", nid + 1, get_sort(sort), nid, get_nid(value));
                }
            }
            Node::Next { nid, sort, state, next, name } =>
                println!("{} next {} {} {} {}", nid, get_sort(sort), get_nid(state), get_nid(next), name.as_ref().map_or("?", |s| &*s)),
            Node::Bad { nid, cond, name } =>
                println!("{} bad {} {}", nid, get_nid(cond), name.as_ref().map_or("?", |s| &*s)),
            Node::Comment(s) =>
                println!("\n; {}\n", s),
        }
    }
    println!("\n; end of BTOR2 file");
}

//
// Private Implementation
//

fn get_nid(node: &NodeRef) -> Nid {
    match *node.borrow() {
        Node::Const { nid, .. } => nid,
        Node::Read { nid, .. } => nid,
        Node::Write { nid, .. } => nid,
        Node::Add { nid, .. } => nid,
        Node::Sub { nid, .. } => nid,
        Node::Rem { nid, .. } => nid,
        Node::Ite { nid, .. } => nid,
        Node::Eq { nid, .. } => nid,
        Node::And { nid, .. } => nid,
        Node::Not { nid, .. } => nid,
        Node::State { nid, .. } => nid,
        Node::Next { nid, .. } => nid,
        Node::Bad { nid, .. } => nid,
        Node::Comment(_) => panic!("has no nid"),
    }
}

fn get_sort(sort: &NodeType) -> Nid {
    match *sort {
        NodeType::Bit => 1,
        NodeType::Word => 2,
        NodeType::Memory => 3,
    }
}

const INSTRUCTION_SIZE: u64 = riscu::INSTRUCTION_SIZE as u64;
const NUMBER_OF_REGISTERS: usize = 32;

impl Eq for HashableNodeRef {}

impl From<NodeRef> for HashableNodeRef {
    fn from(node: NodeRef) -> Self {
        Self { value: node }
    }
}

impl Hash for HashableNodeRef {
    fn hash<H: Hasher>(&self, state: &mut H) {
        RefCell::as_ptr(&self.value).hash(state);
    }
}

impl PartialEq for HashableNodeRef {
    fn eq(&self, other: &Self) -> bool {
        RefCell::as_ptr(&self.value) == RefCell::as_ptr(&other.value)
    }
}

// TODO: Add implementation of all syscalls.
// TODO: Add initialization of memory (data, heap, stack segments).
// TODO: Add support for input variables (used by `read` syscall).
// TODO: Fix initialization of `current_nid` based on binary size.
// TODO(test): Test model by adding a poor-man's type-checker.
// TODO(test): Test builder on existing samples automatically.
struct ModelBuilder {
    lines: LinkedList<NodeRef>,
    sequentials: Vec<NodeRef>,
    bad_states: Vec<NodeRef>,
    pc_flags: HashMap<u64, NodeRef>,
    control_in: HashMap<u64, Vec<InEdge>>,
    zero_bit: NodeRef,
    one_bit: NodeRef,
    zero_word: NodeRef,
    kernel_mode: NodeRef,
    kernel_not: NodeRef,
    register_nodes: Vec<NodeRef>,
    register_flow: Vec<NodeRef>,
    memory_node: NodeRef,
    memory_flow: NodeRef,
    division_flow: NodeRef,
    remainder_flow: NodeRef,
    ecall_flow: NodeRef,
    current_nid: Nid,
    pc: u64,
}

struct InEdge {
    from_instruction: Instruction,
    from_address: u64,
    condition: Option<NodeRef>,
}

impl ModelBuilder {
    fn new() -> Self {
        let dummy_node = Rc::new(RefCell::new(Node::Comment("dummy".to_string())));
        Self {
            lines: LinkedList::new(),
            sequentials: Vec::new(),
            bad_states: Vec::new(),
            pc_flags: HashMap::new(),
            control_in: HashMap::new(),
            zero_bit: dummy_node.clone(),
            one_bit: dummy_node.clone(),
            zero_word: dummy_node.clone(),
            kernel_mode: dummy_node.clone(),
            kernel_not: dummy_node.clone(),
            register_nodes: Vec::with_capacity(NUMBER_OF_REGISTERS),
            register_flow: Vec::with_capacity(NUMBER_OF_REGISTERS - 1),
            memory_node: dummy_node.clone(),
            memory_flow: dummy_node.clone(),
            division_flow: dummy_node.clone(),
            remainder_flow: dummy_node.clone(),
            ecall_flow: dummy_node,
            current_nid: 0,
            pc: 0,
        }
    }

    fn finalize(self) -> Model {
        Model {
            lines: self.lines,
            sequentials: self.sequentials,
            bad_states: self.bad_states,
        }
    }

    fn pc_add(&self, imm: u64) -> u64 {
        self.pc.wrapping_add(imm)
    }

    fn pc_flag(&self) -> NodeRef {
        self.pc_flags[&self.pc].clone()
    }

    fn reg_node(&self, reg: Register) -> NodeRef {
        self.register_nodes[reg as usize].clone()
    }

    fn reg_flow(&self, reg: Register) -> NodeRef {
        assert!(reg != Register::Zero);
        self.register_flow[reg as usize - 1].clone()
    }

    fn reg_flow_update(&mut self, reg: Register, node: NodeRef) {
        assert!(reg != Register::Zero);
        self.register_flow[reg as usize - 1] = node;
    }

    fn add_node(&mut self, node_data: Node) -> NodeRef {
        let node = Rc::new(RefCell::new(node_data));
        self.lines.push_back(node.clone());
        self.current_nid += 1;
        node
    }

    fn new_const(&mut self, imm: u64) -> NodeRef {
        self.add_node(Node::Const {
            nid: self.current_nid,
            sort: NodeType::Word,
            imm,
        })
    }

    fn new_read(&mut self, address: NodeRef) -> NodeRef {
        self.add_node(Node::Read {
            nid: self.current_nid,
            memory: self.memory_node.clone(),
            address,
        })
    }

    fn new_write(&mut self, address: NodeRef, value: NodeRef) -> NodeRef {
        self.add_node(Node::Write {
            nid: self.current_nid,
            memory: self.memory_node.clone(),
            address,
            value,
        })
    }

    fn new_add(&mut self, left: NodeRef, right: NodeRef) -> NodeRef {
        self.add_node(Node::Add {
            nid: self.current_nid,
            left,
            right,
        })
    }

    fn new_sub(&mut self, left: NodeRef, right: NodeRef) -> NodeRef {
        self.add_node(Node::Sub {
            nid: self.current_nid,
            left,
            right,
        })
    }

    fn new_rem(&mut self, left: NodeRef, right: NodeRef) -> NodeRef {
        self.add_node(Node::Rem {
            nid: self.current_nid,
            left,
            right,
        })
    }

    fn new_ite(&mut self, cond: NodeRef, left: NodeRef, right: NodeRef, sort: NodeType) -> NodeRef {
        self.add_node(Node::Ite {
            nid: self.current_nid,
            sort,
            cond,
            left,
            right,
        })
    }

    fn new_eq(&mut self, left: NodeRef, right: NodeRef) -> NodeRef {
        self.add_node(Node::Eq {
            nid: self.current_nid,
            left,
            right,
        })
    }

    fn new_neq(&mut self, left: NodeRef, right: NodeRef) -> NodeRef {
        let eq_node = self.new_eq(left, right);
        self.new_not(eq_node)
    }

    fn new_and(&mut self, left: NodeRef, right: NodeRef) -> NodeRef {
        self.add_node(Node::And {
            nid: self.current_nid,
            left,
            right,
        })
    }

    fn new_not(&mut self, value: NodeRef) -> NodeRef {
        self.add_node(Node::Not {
            nid: self.current_nid,
            value,
        })
    }

    fn new_state(
        &mut self,
        init: Option<NodeRef>,
        name: Option<String>,
        sort: NodeType,
    ) -> NodeRef {
        let nid_increase = if init.is_some() { 1 } else { 0 };
        let state_node = self.add_node(Node::State {
            nid: self.current_nid,
            sort,
            init,
            name,
        });
        self.current_nid += nid_increase;
        state_node
    }

    fn new_next(
        &mut self,
        state: NodeRef,
        next: NodeRef,
        name: Option<String>,
        sort: NodeType,
    ) -> NodeRef {
        let next_node = self.add_node(Node::Next {
            nid: self.current_nid,
            sort,
            state,
            next,
            name,
        });
        self.sequentials.push(next_node.clone());
        next_node
    }

    fn new_bad(&mut self, cond: NodeRef, name: Option<String>) -> NodeRef {
        let bad_node = self.add_node(Node::Bad {
            nid: self.current_nid,
            cond,
            name,
        });
        self.bad_states.push(bad_node.clone());
        bad_node
    }

    fn new_comment(&mut self, s: String) {
        let node = Rc::new(RefCell::new(Node::Comment(s)));
        self.lines.push_back(node);
    }

    fn go_to_instruction(
        &mut self,
        from_instruction: Instruction,
        from_address: u64,
        to_address: u64,
        condition: Option<NodeRef>,
    ) {
        self.control_in
            .entry(to_address)
            .or_insert_with(|| Vec::with_capacity(1))
            .push(InEdge {
                from_instruction,
                from_address,
                condition,
            });
    }

    fn model_addi(&mut self, itype: IType) {
        if itype.rd() == Register::Zero {
            return;
        };
        let imm = itype.imm() as u64;
        let result_node = if imm == 0 {
            self.reg_node(itype.rs1())
        } else if itype.rs1() == Register::Zero {
            self.new_const(imm)
        } else {
            let imm_node = self.new_const(imm);
            self.new_add(self.reg_node(itype.rs1()), imm_node)
        };
        let ite_node = self.new_ite(
            self.pc_flag(),
            result_node,
            self.reg_flow(itype.rd()),
            NodeType::Word,
        );
        self.reg_flow_update(itype.rd(), ite_node);
    }

    fn model_lui(&mut self, utype: UType) {
        let imm_shifted = u64::from(utype.imm()) << 12;
        let const_node = self.new_const(imm_shifted);
        let ite_node = self.new_ite(
            self.pc_flag(),
            const_node,
            self.reg_flow(utype.rd()),
            NodeType::Word,
        );
        self.reg_flow_update(utype.rd(), ite_node);
    }

    fn model_address(&mut self, reg: Register, imm: u64) -> NodeRef {
        if imm == 0 {
            self.reg_node(reg)
        } else {
            let imm_node = self.new_const(imm);
            self.new_add(self.reg_node(reg), imm_node)
        }
    }

    fn model_ld(&mut self, itype: IType) {
        // TODO: Add proper memory-bounds checks.
        let address_node = self.model_address(itype.rs1(), itype.imm() as u64);
        let read_node = self.new_read(address_node);
        let ite_node = self.new_ite(
            self.pc_flag(),
            read_node,
            self.reg_flow(itype.rd()),
            NodeType::Word,
        );
        self.reg_flow_update(itype.rd(), ite_node);
    }

    fn model_sd(&mut self, stype: SType) {
        // TODO: Add proper memory-bounds checks.
        let address_node = self.model_address(stype.rs1(), stype.imm() as u64);
        let write_node = self.new_write(address_node, self.reg_node(stype.rs2()));
        let ite_node = self.new_ite(
            self.pc_flag(),
            write_node,
            self.memory_flow.clone(),
            NodeType::Memory,
        );
        self.memory_flow = ite_node;
    }

    fn model_add(&mut self, rtype: RType) {
        let add_node = self.new_add(self.reg_node(rtype.rs1()), self.reg_node(rtype.rs2()));
        let ite_node = self.new_ite(
            self.pc_flag(),
            add_node,
            self.reg_flow(rtype.rd()),
            NodeType::Word,
        );
        self.reg_flow_update(rtype.rd(), ite_node);
    }

    fn model_sub(&mut self, rtype: RType) {
        let sub_node = self.new_sub(self.reg_node(rtype.rs1()), self.reg_node(rtype.rs2()));
        let ite_node = self.new_ite(
            self.pc_flag(),
            sub_node,
            self.reg_flow(rtype.rd()),
            NodeType::Word,
        );
        self.reg_flow_update(rtype.rd(), ite_node);
    }

    fn model_remu(&mut self, rtype: RType) {
        self.remainder_flow = self.new_ite(
            self.pc_flag(),
            self.reg_node(rtype.rs2()),
            self.remainder_flow.clone(),
            NodeType::Word,
        );
        let rem_node = self.new_rem(self.reg_node(rtype.rs1()), self.reg_node(rtype.rs2()));
        let ite_node = self.new_ite(
            self.pc_flag(),
            rem_node,
            self.reg_flow(rtype.rd()),
            NodeType::Word,
        );
        self.reg_flow_update(rtype.rd(), ite_node);
    }

    fn model_beq(
        &mut self,
        btype: BType,
        out_true: &mut Option<NodeRef>,
        out_false: &mut Option<NodeRef>,
    ) {
        let cond_true = self.new_eq(self.reg_node(btype.rs1()), self.reg_node(btype.rs2()));
        let cond_false = self.new_not(cond_true.clone());
        out_true.replace(cond_true);
        out_false.replace(cond_false);
    }

    fn model_jal(&mut self, jtype: JType) {
        if jtype.rd() == Register::Zero {
            return;
        };
        let link_node = self.new_const(self.pc_add(INSTRUCTION_SIZE));
        let ite_node = self.new_ite(
            self.pc_flag(),
            link_node,
            self.reg_flow(jtype.rd()),
            NodeType::Word,
        );
        self.reg_flow_update(jtype.rd(), ite_node);
    }

    fn model_ecall(&mut self) {
        let ite_node = self.new_ite(
            self.pc_flag(),
            self.one_bit.clone(),
            self.ecall_flow.clone(),
            NodeType::Bit,
        );
        self.ecall_flow = ite_node;
    }

    fn translate_to_model(&mut self, inst: Instruction) {
        let mut beq_true = None;
        let mut beq_false = None;

        match inst {
            Instruction::Addi(itype) => self.model_addi(itype),
            Instruction::Lui(utype) => self.model_lui(utype),
            Instruction::Ld(itype) => self.model_ld(itype),
            Instruction::Sd(stype) => self.model_sd(stype),
            Instruction::Add(rtype) => self.model_add(rtype),
            Instruction::Sub(rtype) => self.model_sub(rtype),
            Instruction::Mul(_rtype) => panic!("implement MUL"),
            Instruction::Divu(_rtype) => panic!("implement DIV"),
            Instruction::Remu(rtype) => self.model_remu(rtype),
            Instruction::Sltu(_rtype) => panic!("implement SLTU"),
            Instruction::Beq(btype) => self.model_beq(btype, &mut beq_true, &mut beq_false),
            Instruction::Jal(jtype) => self.model_jal(jtype),
            Instruction::Jalr(_itype) => {} // TODO: Implement me!
            Instruction::Ecall(_) => self.model_ecall(),
        }

        match inst {
            Instruction::Addi(_)
            | Instruction::Lui(_)
            | Instruction::Ld(_)
            | Instruction::Sd(_)
            | Instruction::Add(_)
            | Instruction::Sub(_)
            | Instruction::Mul(_)
            | Instruction::Divu(_)
            | Instruction::Remu(_)
            | Instruction::Sltu(_) => {
                self.go_to_instruction(inst, self.pc, self.pc_add(INSTRUCTION_SIZE), None);
            }
            Instruction::Beq(btype) => {
                self.go_to_instruction(inst, self.pc, self.pc_add(btype.imm() as u64), beq_true);
                self.go_to_instruction(inst, self.pc, self.pc_add(INSTRUCTION_SIZE), beq_false);
            }
            Instruction::Jal(jtype) => {
                self.go_to_instruction(inst, self.pc, self.pc_add(jtype.imm() as u64), None);
            }
            Instruction::Jalr(itype) => {
                assert_eq!(itype.rd(), Register::Zero);
                assert_eq!(itype.rs1(), Register::Ra);
                assert_eq!(itype.imm(), 0);
            }
            Instruction::Ecall(_) => {
                self.go_to_instruction(inst, self.pc, self.pc_add(INSTRUCTION_SIZE), None);
            }
        }
    }

    fn control_flow_from_beq(&mut self, edge: &InEdge, flow: NodeRef) -> NodeRef {
        let and_node = self.new_and(
            self.pc_flags[&edge.from_address].clone(),
            edge.condition.as_ref().expect("must exist").clone(),
        );
        self.control_flow_from_any(and_node, flow)
    }

    fn control_flow_from_ecall(&mut self, edge: &InEdge, flow: NodeRef) -> NodeRef {
        let state_node = self.new_state(
            Some(self.zero_bit.clone()),
            Some(format!("kernel-mode-pc-flag-{}", edge.from_address)),
            NodeType::Bit,
        );
        let ite_node = self.new_ite(
            state_node.clone(),
            self.kernel_mode.clone(),
            self.pc_flags[&edge.from_address].clone(),
            NodeType::Bit,
        );
        self.new_next(state_node.clone(), ite_node, None, NodeType::Bit);
        let and_node = self.new_and(state_node, self.kernel_not.clone());
        self.control_flow_from_any(and_node, flow)
    }

    fn control_flow_from_any(&mut self, activate: NodeRef, flow: NodeRef) -> NodeRef {
        if Rc::ptr_eq(&flow, &self.zero_bit) {
            return activate;
        }
        self.new_ite(activate, self.one_bit.clone(), flow, NodeType::Bit)
    }

    fn generate_model(&mut self, program: &Program) -> Result<()> {
        self.new_comment("common constants".to_string());
        self.zero_bit = self.add_node(Node::Const {
            nid: 10,
            sort: NodeType::Bit,
            imm: 0,
        });
        self.one_bit = self.add_node(Node::Const {
            nid: 11,
            sort: NodeType::Bit,
            imm: 1,
        });
        self.zero_word = self.add_node(Node::Const {
            nid: 20,
            sort: NodeType::Word,
            imm: 0,
        });
        self.division_flow = self.add_node(Node::Const {
            nid: 21,
            sort: NodeType::Word,
            imm: 1,
        });
        self.remainder_flow = self.division_flow.clone();
        self.ecall_flow = self.zero_bit.clone();

        self.new_comment("kernel-mode flag".to_string());
        self.current_nid = 60;
        self.kernel_mode = self.new_state(
            Some(self.zero_bit.clone()),
            Some("kernel-mode".to_string()),
            NodeType::Bit,
        );
        self.kernel_not = self.new_not(self.kernel_mode.clone());

        self.new_comment("32 64-bit general-purpose registers".to_string());
        self.current_nid = 200;
        let zero_register = self.new_const(0);
        self.register_nodes.push(zero_register);
        for r in 1..NUMBER_OF_REGISTERS {
            let reg = Register::from(r as u32);
            self.current_nid = 200 + 2 * r as u64;
            let register_node = self.new_state(
                Some(self.zero_word.clone()),
                Some(format!("{:?}", reg)),
                NodeType::Word,
            );
            self.register_nodes.push(register_node.clone());
            self.register_flow.push(register_node);
        }
        assert_eq!(self.register_nodes.len(), NUMBER_OF_REGISTERS);
        assert_eq!(self.register_flow.len(), NUMBER_OF_REGISTERS - 1);

        self.new_comment("64-bit program counter encoded in Boolean flags".to_string());
        self.pc = program.code.address;
        for n in (0..program.code.content.len()).step_by(size_of::<u32>()) {
            self.current_nid = 10000000 + self.pc * 100;
            let init = if n == 0 {
                self.one_bit.clone()
            } else {
                self.zero_bit.clone()
            };
            let flag_node = self.new_state(
                Some(init),
                Some(format!("pc={:#x}", self.pc)),
                NodeType::Bit,
            );
            self.pc_flags.insert(self.pc, flag_node);
            self.pc = self.pc_add(INSTRUCTION_SIZE);
        }

        self.new_comment("64-bit virtual memory".to_string());
        self.current_nid = 20000000;
        let memory_dump = self.new_state(None, Some("memory-dump".to_string()), NodeType::Memory);
        let memory_node = self.new_state(
            Some(memory_dump),
            Some("virtual-memory".to_string()),
            NodeType::Memory,
        );
        self.memory_node = memory_node.clone();
        self.memory_flow = memory_node;

        self.new_comment("data flow".to_string());
        self.pc = program.code.address;
        program
            .code
            .content
            .chunks_exact(size_of::<u32>())
            .map(LittleEndian::read_u32)
            .try_for_each(|raw| {
                decode(raw).map(|inst| {
                    self.current_nid = 30000000 + self.pc * 100;
                    self.translate_to_model(inst);
                    self.pc = self.pc_add(INSTRUCTION_SIZE);
                })
            })
            .context("Failed to decode instructions of program")?;

        self.new_comment("syscalls".to_string());
        self.current_nid = 40000000;
        let num_openat = self.new_const(SyscallId::Openat as u64);
        let is_openat = self.new_eq(self.reg_node(Register::A7), num_openat);
        let num_read = self.new_const(SyscallId::Read as u64);
        let is_read = self.new_eq(self.reg_node(Register::A7), num_read);
        let num_write = self.new_const(SyscallId::Write as u64);
        let is_write = self.new_eq(self.reg_node(Register::A7), num_write);
        let num_exit = self.new_const(SyscallId::Exit as u64);
        let is_exit = self.new_eq(self.reg_node(Register::A7), num_exit);
        let num_brk = self.new_const(SyscallId::Brk as u64);
        let is_brk = self.new_eq(self.reg_node(Register::A7), num_brk);
        let active_exit = self.new_and(self.ecall_flow.clone(), is_exit.clone());
        let kernel_flow = self.new_ite(
            self.kernel_mode.clone(),
            is_exit.clone(),
            active_exit.clone(),
            NodeType::Bit,
        );
        self.new_next(self.kernel_mode.clone(), kernel_flow, None, NodeType::Bit);

        self.new_comment("control flow".to_string());
        self.pc = program.code.address;
        for _n in (0..program.code.content.len()).step_by(size_of::<u32>()) {
            self.current_nid = 50000000 + self.pc * 100;
            let mut control_flow = self.zero_bit.clone();
            for in_edge in self.control_in.remove(&self.pc).unwrap_or_default() {
                control_flow = match in_edge.from_instruction {
                    Instruction::Beq(_) => self.control_flow_from_beq(&in_edge, control_flow),
                    Instruction::Ecall(_) => self.control_flow_from_ecall(&in_edge, control_flow),
                    _ => self.control_flow_from_any(
                        self.pc_flags[&in_edge.from_address].clone(),
                        control_flow,
                    ),
                }
            }
            self.new_next(self.pc_flag(), control_flow, None, NodeType::Bit);
            self.pc = self.pc_add(INSTRUCTION_SIZE);
        }

        self.new_comment("updating registers".to_string());
        for r in 1..NUMBER_OF_REGISTERS {
            self.current_nid = 60000000 + (r as u64);
            let reg = Register::from(r as u32);
            self.new_next(
                self.reg_node(reg),
                self.reg_flow(reg),
                Some(format!("{:?}", reg)),
                NodeType::Word,
            );
        }

        self.new_comment("updating 64-bit virtual memory".to_string());
        self.current_nid = 70000000;
        self.new_next(
            self.memory_node.clone(),
            self.memory_flow.clone(),
            Some("virtual-memory".to_string()),
            NodeType::Memory,
        );

        self.new_comment("checking syscall id".to_string());
        self.current_nid = 80000000;
        let not_openat = self.new_not(is_openat);
        let not_read = self.new_not(is_read);
        let not_write = self.new_not(is_write);
        let not_exit = self.new_not(is_exit);
        let not_brk = self.new_not(is_brk);
        let check_syscall_and1 = self.new_and(not_openat, not_read);
        let check_syscall_and2 = self.new_and(check_syscall_and1, not_write);
        let check_syscall_and3 = self.new_and(check_syscall_and2, not_exit);
        let check_syscall_and4 = self.new_and(check_syscall_and3, not_brk);
        let check_syscall = self.new_and(self.ecall_flow.clone(), check_syscall_and4);
        self.new_bad(check_syscall, Some("invalid-syscall-id".to_string()));

        self.new_comment("checking exit code".to_string());
        let check_exit_code = self.new_neq(self.reg_node(Register::A0), self.zero_word.clone());
        let check_exit = self.new_and(active_exit, check_exit_code);
        self.new_bad(check_exit, Some("non-zero-exit-code".to_string()));

        self.new_comment("checking division and remainder by zero".to_string());
        let check_div = self.new_eq(self.division_flow.clone(), self.zero_word.clone());
        self.new_bad(check_div, Some("bad-division-by-zero".to_string()));
        let check_rem = self.new_eq(self.remainder_flow.clone(), self.zero_word.clone());
        self.new_bad(check_rem, Some("bad-remainder-by-zero".to_string()));

        Ok(())
    }
}
