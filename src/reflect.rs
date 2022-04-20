//! Reflection procedures and types.
use std::convert::{TryFrom};
use std::iter::Peekable;
use std::ops::RangeInclusive;
use std::fmt;
use std::mem::transmute;
use fnv::{FnvHashMap as HashMap, FnvHashSet as HashSet};
use crate::ty::*;
use crate::consts::*;
use crate::{EntryPoint, SpirvBinary};
use crate::parse::{Instrs, Instr};
use crate::error::{Error, Result};
use crate::instr::*;
use crate::inspect::{Inspector, NopInspector, FnInspector};
use crate::walk::Walk;

use spirv_headers::Dim;
pub use spirv_headers::{ExecutionModel, Decoration, StorageClass};

// Public types.

/// Descriptor set and binding point carrier.
#[derive(PartialEq, Eq, PartialOrd, Ord, Hash, Default, Clone, Copy)]
pub struct DescriptorBinding(u32, u32);
impl DescriptorBinding {
    pub fn new(desc_set: u32, bind_point: u32) -> Self { DescriptorBinding(desc_set, bind_point) }

    pub fn set(&self) -> u32 { self.0 }
    pub fn bind(&self) -> u32 { self.1 }
    pub fn into_inner(self) -> (u32, u32) { (self.0, self.1) }
}
impl fmt::Display for DescriptorBinding {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            write!(f, "(set={}, bind={})", self.0, self.1)
    }
}
impl fmt::Debug for DescriptorBinding {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { (self as &dyn fmt::Display).fmt(f) }
}

/// Interface variable location and component.
#[derive(PartialEq, Eq, PartialOrd, Ord, Hash, Default, Clone, Copy)]
pub struct InterfaceLocation(u32, u32);
impl InterfaceLocation {
    pub fn new(loc: u32, comp: u32) -> Self { InterfaceLocation(loc, comp) }

    pub fn loc(&self) -> u32 { self.0 }
    pub fn comp(&self) -> u32 { self.1 }
    pub fn into_inner(self) -> (u32, u32) { (self.0, self.1) }
}
impl fmt::Display for InterfaceLocation {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "(loc={}, comp={})", self.0, self.1)
    }
}
impl fmt::Debug for InterfaceLocation {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { (self as &dyn fmt::Display).fmt(f) }
}

/// Specialization constant ID.
pub type SpecId = u32;
#[derive(Clone, Copy, Debug)]
pub enum ConstantValue {
    /// Logical boolean value.
    Bool(bool),
    /// Signed 32-bit integer.
    I32(i32),
    /// Signless 32-bit integer. Note that 'signless' is not 'unsigned'. It
    /// means that SPIR-V integers don't have any sign semantics themselves, the
    /// significance of the sign-bit depends on the signess of the operation
    /// applied to it.
    U32(u32),
    /// Signed 32-bit floating-point number.
    F32(f32),
    /// Signed 64-bit integer.
    I64(i64),
    /// Signless 64-bit integer.
    U64(u64),
    /// Signed 64-bit floating-point number.
    F64(f64),
}
impl From<bool> for ConstantValue {
    fn from(x: bool) -> Self { ConstantValue::Bool(x) }
}
impl From<u32> for ConstantValue {
    fn from(x: u32) -> Self { ConstantValue::U32(x) }
}
impl From<i32> for ConstantValue {
    fn from(x: i32) -> Self { ConstantValue::I32(x) }
}
impl From<f32> for ConstantValue {
    fn from(x: f32) -> Self { ConstantValue::F32(x) }
}
impl From<u64> for ConstantValue {
    fn from(x: u64) -> Self { ConstantValue::U64(x) }
}
impl From<i64> for ConstantValue {
    fn from(x: i64) -> Self { ConstantValue::I64(x) }
}
impl From<f64> for ConstantValue {
    fn from(x: f64) -> Self { ConstantValue::F64(x) }
}
impl ConstantValue {
    fn to_s32(&self) -> Result<i32> {
        match self {
            ConstantValue::I32(x) => Ok(*x),
            ConstantValue::U32(x) => Ok(unsafe { transmute::<u32, i32>(*x) }),
            _ => Err(Error::SPEC_TY_MISMATCHED),
        }
    }
    fn to_u32(&self) -> Result<u32> {
        match self {
            ConstantValue::I32(x) => Ok(unsafe { transmute::<i32, u32>(*x) }),
            ConstantValue::U32(x) => Ok(*x),
            _ => Err(Error::SPEC_TY_MISMATCHED),
        }
    }

    fn ty(&self) -> Type {
        match self {
            Self::Bool(_) => Type::Scalar(ScalarType::Boolean),
            Self::I32(_) => Type::Scalar(ScalarType::Signed(4)),
            Self::U32(_) => Type::Scalar(ScalarType::Unsigned(4)),
            Self::F32(_) => Type::Scalar(ScalarType::Float(4)),
            Self::I64(_) => Type::Scalar(ScalarType::Signed(8)),
            Self::U64(_) => Type::Scalar(ScalarType::Unsigned(8)),
            Self::F64(_) => Type::Scalar(ScalarType::Float(8)),
        }
    }
}

/// Variable locator.
#[derive(PartialEq, Eq, Hash, Clone, Copy, Debug)]
pub enum Locator {
    Input(InterfaceLocation),
    Output(InterfaceLocation),
    Descriptor(DescriptorBinding),
    PushConstant,
    SpecConstant(SpecId),
}


// Intermediate types used in reflection.

/// Reflection intermediate of constants and specialization constant.
#[derive(Debug, Clone)]
pub struct ConstantIntermediate {
    /// Defined value of constant, or default value of specialization constant.
    pub value: ConstantValue,
    /// Specialization constant ID, notice that this is NOT an instruction ID.
    /// It is used to identify specialization constants for graphics libraries.
    pub spec_id: Option<SpecId>,
}

/// Descriptor type matching `VkDescriptorType`.
#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub enum DescriptorType {
    /// `VK_DESCRIPTOR_TYPE_SAMPLER`
    Sampler(),
    /// `VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER`
    CombinedImageSampler(),
    /// `VK_DESCRIPTOR_TYPE_SAMPLED_IMAGE`
    SampledImage(),
    /// `VK_DESCRIPTOR_TYPE_STORAGE_IMAGE`
    StorageImage(AccessType),
    /// `VK_DESCRIPTOR_TYPE_UNIFORM_TEXEL_BUFFER`.
    UniformTexelBuffer(),
    /// `VK_DESCRIPTOR_TYPE_STORAGE_TEXEL_BUFFER`.
    StorageTexelBuffer(AccessType),
    /// `VK_DESCRIPTOR_TYPE_UNIFORM_BUFFER` or
    /// `VK_DESCRIPTOR_TYPE_UNIFORM_BUFFER_DYNAMIC` depending on how you gonna
    /// use it.
    UniformBuffer(),
    /// `VK_DESCRIPTOR_TYPE_STORAGE_BUFFER` or
    /// `VK_DESCRIPTOR_TYPE_STORAGE_BUFFER_DYNAMIC` depending on how you gonna
    /// use it.
    StorageBuffer(AccessType),
    /// `VK_DESCRIPTOR_TYPE_INPUT_ATTACHMENT` and its input attachment index.
    InputAttachment(u32),
    /// `VK_DESCRIPTOR_TYPE_ACCELERATION_STRUCTURE_KHR`
    AccelStruct(),
}

/// A SPIR-V variable - interface variables, descriptor resources and push
/// constants.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Variable {
    /// Input interface variable.
    Input {
        name: Option<String>,
        // Interface location of input.
        location: InterfaceLocation,
        /// The concrete SPIR-V type definition of descriptor resource.
        ty: Type,
    },
    /// Output interface variable.
    Output {
        name: Option<String>,
        // Interface location of output.
        location: InterfaceLocation,
        /// The concrete SPIR-V type definition of descriptor resource.
        ty: Type,
    },
    /// Descriptor resource.
    Descriptor {
        name: Option<String>,
        // Binding point of descriptor resource.
        desc_bind: DescriptorBinding,
        /// Descriptor resource type matching `VkDescriptorType`.
        desc_ty: DescriptorType,
        /// The concrete SPIR-V type definition of descriptor resource.
        ty: Type,
        /// Number of bindings at the binding point. All descriptors can have
        /// multiple binding points. If the multi-binding is dynamic, 0 will be
        /// returned.
        ///
        /// For more information about dynamic multi-binding, please refer to
        /// Vulkan extension `VK_EXT_descriptor_indexing`, GLSL extension
        /// `GL_EXT_nonuniform_qualifier` and SPIR-V extension
        /// `SPV_EXT_descriptor_indexing`. Dynamic multi-binding is only
        /// supported in Vulkan 1.2.
        nbind: u32,
    },
    /// Push constant.
    PushConstant {
        name: Option<String>,
        /// The concrete SPIR-V type definition of descriptor resource.
        ty: Type,
    },
    /// Specialization constant.
    SpecConstant {
        name: Option<String>,
        /// Specialization constant ID.
        spec_id: SpecId,
        /// The type of the specialization constant.
        ty: Type,
    }
}
impl Variable {
    /// Debug name of this variable.
    pub fn name(&self) -> Option<&str> {
        match self {
            Variable::Input { name, .. } => name.as_ref().map(|x| x as &str),
            Variable::Output { name, .. } => name.as_ref().map(|x| x as &str),
            Variable::Descriptor { name, .. } => name.as_ref().map(|x| x as &str),
            Variable::PushConstant { name, .. } => name.as_ref().map(|x| x as &str),
            Variable::SpecConstant { name, .. } => name.as_ref().map(|x| x as &str),
        }
    }
    /// Remove name of the variable.
    pub fn clear_name(&mut self) {
        match self {
            Variable::Input { name, .. } => *name = None,
            Variable::Output { name, .. } => *name = None,
            Variable::Descriptor { name, .. } => *name = None,
            Variable::PushConstant { name, .. } => *name = None,
            Variable::SpecConstant { name, .. } => *name = None,
        }
    }
    /// Locator of the variable.
    pub fn locator(&self) -> Locator {
        match self {
            Variable::Input { location, .. } => Locator::Input(*location),
            Variable::Output { location, .. } => Locator::Output(*location),
            Variable::Descriptor { desc_bind, .. } => Locator::Descriptor(*desc_bind),
            Variable::PushConstant { .. } => Locator::PushConstant,
            Variable::SpecConstant { spec_id, .. } => Locator::SpecConstant(*spec_id),
        }
    }
    /// Descriptor type if it's a descriptor resource.
    pub fn desc_ty(&self) -> Option<DescriptorType> {
        if let Variable::Descriptor { desc_ty, .. } = self {
            Some(desc_ty.clone())
        } else { None }
    }
    /// Specialization constant ID if it's a specialization constant.
    pub fn spec_id(&self) -> Option<SpecId> {
        if let Variable::SpecConstant { spec_id, .. } = self {
            Some(*spec_id)
        } else { None }
    }
    /// Concrete type of the variable.
    pub fn ty(&self) -> &Type {
        match self {
            Variable::Input { ty, .. } => ty,
            Variable::Output { ty, .. } => ty,
            Variable::Descriptor { ty, .. } => ty,
            Variable::PushConstant { ty, .. } => ty,
            Variable::SpecConstant { ty, .. } => ty,
        }
    }
    /// Number of bindings at the binding point it it's a descriptor resource.
    pub fn nbind(&self) -> Option<u32> {
        if let Variable::Descriptor { nbind, .. } = self {
            Some(*nbind)
        } else { None }
    }
    /// Enumerate variable members in post-order.
    pub fn walk<'a>(&'a self) -> Walk<'a> {
        self.ty().walk()
    }
}
/// Function reflection intermediate.
#[derive(Default, Debug, Clone)]
pub struct FunctionIntermediate {
    pub accessed_vars: HashSet<VariableId>,
    pub callees: HashSet<InstrId>,
}
struct EntryPointDeclartion<'a> {
    pub func_id: FunctionId,
    pub name: &'a str,
    pub exec_model: ExecutionModel,
}
/// SPIR-V execution mode.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum ExecutionMode {
    /// Number of times to invoke the geometry stage for each input primitive
    /// received. The default is to run once for each input primitive. It is
    /// invalid to specify a value greater than the target-dependent maximum.
    ///
    /// Only valid with the geometry execution model.
    Invocations(u32),
    /// Requests the tesselation primitive generator to divide edges into a
    /// collection of equal-sized segments.
    ///
    /// Only valid with one of the tessellation execution models.
    SpacingEqual,
    /// Requests the tessellation primitive generator to divide edges into an
    /// even number of equal-length segments plus two additional shorter
    /// fractional segments.
    ///
    /// Only valid with one of the tessellation execution models.
    SpacingFractionalEven,
    /// Requests the tessellation primitive generator to divide edges into an
    /// odd number of equal-length segments plus two additional shorter
    /// fractional segments.
    ///
    /// Only valid with one of the tessellation execution models.
    SpacingFractionalOdd,
    /// Requests the tessellation primitive generator to generate triangles in
    /// clockwise order.
    ///
    /// Only valid with one of the tessellation execution models.
    VertexOrderCw,
    /// Requests the tessellation primitive generator to generate triangles in
    /// counter-clockwise order.
    ///
    /// Only valid with one of the tessellation execution models.
    VertexOrderCcw,
    /// Pixels appear centered on whole-number pixel offsets. E.g., the
    /// coordinate (0.5, 0.5) appears to move to (0.0, 0.0).
    ///
    /// Only valid with the fragment execution model.
    /// If a fragment entry point does not have this set, pixels appear centered
    /// at offsets of (0.5, 0.5) from whole numbers.
    PixelCenterInteger,
    /// Pixel coordinates appear to originate in the upper left, and increase
    /// toward the right and downward.
    ///
    /// Only valid with the fragment execution model.
    OriginUpperLeft,
    /// Pixel coordinates appear to originate in the lower left, and increase
    /// toward the right and upward.
    ///
    /// Only valid with the fragment execution model.
    OriginLowerLeft,
    /// Fragment tests are to be performed before fragment shader execution.
    ///
    /// Only valid with the fragment execution model.
    EarlyFragmentTests,
    /// Requests the tessellation primitive generator to generate a point for
    /// each distinct vertex in the subdivided primitive, rather than to
    /// generate lines or triangles.
    ///
    /// Only valid with one of the tessellation execution models.
    PointMode,
    /// This stage will run in transform feedback-capturing mode and this module
    /// is responsible for describing the transform-feedback setup.
    ///
    /// See the XfbBuffer, Offset, and XfbStride decorations.
    Xfb,
    /// This mode must be declared if this module potentially changes the
    /// fragment’s depth.
    ///
    /// Only valid with the fragment execution model.
    DepthReplacing,
    /// External optimizations may assume depth modifications will leave the
    /// fragment’s depth as greater than or equal to the fragment’s interpolated
    /// depth value (given by the z component of the FragCoord BuiltIn decorated
    /// variable).
    ///
    /// Only valid with the fragment execution model.
    DepthGreater,
    /// External optimizations may assume depth modifications leave the
    /// fragment’s depth less than the fragment’s interpolated depth value,
    /// (given by the z component of the FragCoord BuiltIn decorated variable).
    ///
    /// Only valid with the fragment execution model.
    DepthLess,
    /// External optimizations may assume this stage did not modify the
    /// fragment’s depth. However, DepthReplacing mode must accurately represent
    /// depth modification.
    ///
    /// Only valid with the fragment execution model.
    DepthUnchanged,
    /// Indicates the work-group size in the x, y, and z dimensions.
    ///
    /// Only valid with the GLCompute or Kernel execution models.
    LocalSize { x: u32, y: u32, z: u32 },
    /// Stage input primitive is points.
    ///
    /// Only valid with the geometry execution model.
    InputPoints,
    /// Stage input primitive is lines.
    ///
    /// Only valid with the geometry execution model.
    InputLines,
    /// Stage input primitive is lines adjacency.
    ///
    /// Only valid with the geometry execution model.
    InputLinesAdjacency,
    /// For a geometry stage, input primitive is triangles. For a tessellation
    /// stage, requests the tessellation primitive generator to generate
    /// triangles.
    ///
    /// Only valid with the geometry or one of the tessellation execution
    /// models.
    Triangles,
    /// Geometry stage input primitive is triangles adjacency.
    ///
    /// Only valid with the geometry execution model.
    InputTrianglesAdjacency,
    /// Requests the tessellation primitive generator to generate quads.
    ///
    /// Only valid with one of the tessellation execution models.
    Quads,
    /// Requests the tessellation primitive generator to generate isolines.
    ///
    /// Only valid with one of the tessellation execution models.
    Isolines,
    /// For a geometry stage, the maximum number of vertices the shader will
    /// ever emit in a single invocation. For a tessellation-control stage, the
    /// number of vertices in the output patch produced by the tessellation
    /// control shader, which also specifies the number of times the
    /// tessellation control shader is invoked.
    ///
    /// Only valid with the geometry or one of the tessellation execution
    /// models.
    OutputVertices(u32),
    /// Stage output primitive is points.
    ///
    /// Only valid with the geometry execution model.
    OutputPoints,
    /// Stage output primitive is line strip.
    ///
    /// Only valid with the geometry execution model.
    OutputLineStrip,
    /// Stage output primitive is triangle strip.
    ///
    /// Only valid with the geometry execution model.
    OutputTriangleStrip,
    /// Indicates that this entry point is a module initializer.
    Initializer,
    /// Indicates that this entry point is a module finalizer.
    Finalizer,
    /// Indicates that this entry point requires the specified Subgroup Size.
    SubgroupSize(u32),
    /// Indicates that this entry point requires the specified number of
    /// Subgroups Per Workgroup.
    SubgroupsPerWorkgroup(u32),
    /// Indicates that this entry point requires the specified number of
    /// Subgroups Per Workgroup.
    ///
    /// Specified as an Id.
    SubgroupsPerWorkgroupId(SpecId),
    /// Indicates the work-group size in the x, y, and z dimensions.
    ///
    /// Only valid with the GLCompute or Kernel execution models.
    ///
    /// Specified as Ids.
    LocalSizeId { x: SpecId, y: SpecId, z: SpecId },
    PostDepthCoverage,
    StencilRefReplacingEXT,
}
struct ExecutionModeDeclaration {
    pub func_id: FunctionId,
    pub execution_mode: ExecutionMode,
}

/// Access type of a variable.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AccessType {
    /// The variable can be accessed by read.
    ReadOnly = 1,
    /// The variable can be accessed by write.
    WriteOnly = 2,
    /// The variable can be accessed by read or by write.
    ReadWrite = 3,
}
impl std::ops::BitOr<AccessType> for AccessType {
    type Output = AccessType;
    fn bitor(self, rhs: AccessType) -> AccessType {
        return match (self, rhs) {
            (Self::ReadOnly, Self::ReadOnly) => Self::ReadOnly,
            (Self::WriteOnly, Self::WriteOnly) => Self::WriteOnly,
            _ => Self::ReadWrite,
        }
    }
}
impl std::ops::BitAnd<AccessType> for AccessType {
    type Output = Option<AccessType>;
    fn bitand(self, rhs: AccessType) -> Option<AccessType> {
        return match (self, rhs) {
            (Self::ReadOnly, Self::ReadWrite) |
                (Self::ReadWrite, Self::ReadOnly) |
                (Self::ReadOnly, Self::ReadOnly) => Some(Self::ReadOnly),
            (Self::WriteOnly, Self::ReadWrite) |
                (Self::ReadWrite, Self::WriteOnly) |
                (Self::WriteOnly, Self::WriteOnly) => Some(Self::WriteOnly),
            (Self::ReadWrite, Self::ReadWrite) => Some(Self::ReadWrite),
            (_, _) => None,
        }
    }
}



// The actual reflection to take place.

/// SPIR-V reflection intermediate.
#[derive(Default)]
pub struct ReflectIntermediate<'a> {
    entry_point_declrs: Vec<EntryPointDeclartion<'a>>,
    execution_mode_declrs: Vec<ExecutionModeDeclaration>,
    vars: Vec<Variable>,

    name_map: HashMap<(InstrId, Option<u32>), &'a str>,
    deco_map: HashMap<(InstrId, Option<u32>, u32), &'a [u32]>,
    ty_map: HashMap<TypeId, Type>,
    var_map: HashMap<VariableId, usize>,
    const_map: HashMap<ConstantId, ConstantIntermediate>,
    ptr_map: HashMap<TypeId, TypeId>,
    func_map: HashMap<FunctionId, FunctionIntermediate>,
    declr_map: HashMap<Locator, InstrId>,
}
impl<'a> ReflectIntermediate<'a> {
    /// Check if a result (like a variable declaration result) or a memeber of a
    /// result (like a structure definition result) has the given decoration.
    pub fn contains_deco(&self, id: InstrId, member_idx: Option<u32>, deco: Decoration) -> bool {
        self.deco_map.contains_key(&(id, member_idx, deco as u32))
    }
    /// Get the single-word decoration of an instruction result.
    pub fn get_deco_u32(&self, id: InstrId, deco: Decoration) -> Option<u32> {
        self.get_deco_list(id, deco)
            .and_then(|x| x.get(0))
            .cloned()
    }
    /// Get the single-word decoration of a member of an instruction result.
    pub fn get_member_deco_u32(
        &self,
        id: InstrId,
        member_idx: u32,
        deco: Decoration,
    ) -> Option<u32> {
        self.get_member_deco_list(id, member_idx, deco)
            .and_then(|x| x.get(0))
            .cloned()
    }
    /// Get the multi-word declaration of a instruction result.
    pub fn get_deco_list(&self, id: InstrId, deco: Decoration) -> Option<&'a [u32]> {
        self.deco_map.get(&(id, None, deco as u32))
            .cloned()
    }
    /// Get the multi-word declaration of a member of an instruction result.
    pub fn get_member_deco_list(
        &self,
        id: InstrId,
        member_idx: u32,
        deco: Decoration,
    ) -> Option<&'a [u32]> {
        self.deco_map.get(&(id, Some(member_idx), deco as u32))
            .cloned()
    }
    /// Get the location-component pair of an interface variable.
    pub fn get_var_location(&self, var_id: VariableId) -> Option<InterfaceLocation> {
        let comp = self.get_deco_u32(var_id, Decoration::Component)
            .unwrap_or(0);
        self.get_deco_u32(var_id, Decoration::Location)
            .map(|loc| InterfaceLocation(loc, comp))
    }
    /// Get the set-binding pair of a descriptor resource.
    pub fn get_var_desc_bind(&self, var_id: VariableId) -> Option<DescriptorBinding> {
        let desc_set = self.get_deco_u32(var_id, Decoration::DescriptorSet)
            .unwrap_or(0);
        self.get_deco_u32(var_id, Decoration::Binding)
            .map(|bind_point| DescriptorBinding::new(desc_set, bind_point))
    }
    /// Get the set-binding pair of a descriptor resource, but the binding point
    /// is forced to 0 if it's not specified in SPIR-V source.
    pub fn get_var_desc_bind_or_default(&self, var_id: VariableId) -> DescriptorBinding {
        self.get_var_desc_bind(var_id)
            .unwrap_or(DescriptorBinding(0, 0))
    }
    /// Get the type identified by `ty_id`.
    pub fn get_ty(&self, ty_id: TypeId) -> Result<&Type> {
        self.ty_map.get(&ty_id)
            .ok_or(Error::TY_NOT_FOUND)
    }
    fn put_ty(&mut self, ty_id: TypeId, ty: Type) -> Result<()> {
        use std::collections::hash_map::Entry::Vacant;
        match self.ty_map.entry(ty_id) {
            Vacant(entry) => {
                entry.insert(ty);
                Ok(())
            },
            _ => Err(Error::ID_COLLISION),
        }
    }
    /// Get the variable identified by `var_id`.
    pub fn get_var(&self, var_id: VariableId) -> Option<&Variable> {
        let ivar = *self.var_map.get(&var_id)?;
        let var = &self.vars[ivar];
        Some(var)
    }
    /// Get the constant identified by `const_id`. Specialization constants are
    /// also stored as constants. Array extents specified by specialization
    /// constants are not statically known.
    pub fn get_const(&self, const_id: ConstantId) -> Result<&ConstantIntermediate> {
        self.const_map.get(&const_id)
            .ok_or(Error::CONST_NOT_FOUND)
    }
    fn put_const(&mut self, const_id: ConstantId, constant: ConstantIntermediate) -> Result<()> {
        use std::collections::hash_map::Entry::Vacant;
        match self.const_map.entry(const_id) {
            Vacant(entry) => {
                entry.insert(constant);
                Ok(())
            },
            _ => Err(Error::ID_COLLISION),
        }
    }
    fn put_bool_const(
        &mut self,
        const_id: ConstantId,
        value: bool,
        spec_id: Option<SpecId>,
    ) -> Result<()> {
        let constant = ConstantIntermediate {
            value: ConstantValue::Bool(value),
            spec_id,
        };
        self.put_const(const_id, constant)
    }
    fn put_lit_const(
        &mut self,
        const_id: ConstantId,
        ty_id: TypeId,
        value: &'_[u32],
        spec_id: Option<SpecId>,
    ) -> Result<()> {
        let value = match self.get_ty(ty_id)? {
            Type::Scalar(ScalarType::Unsigned(4)) if value.len() == 1 => {
                ConstantValue::U32(unsafe { transmute(value[0]) })
            },
            Type::Scalar(ScalarType::Signed(4)) if value.len() == 1 => {
                ConstantValue::I32(unsafe { transmute(value[0]) })
            },
            Type::Scalar(ScalarType::Float(4)) if value.len() == 1 => {
                ConstantValue::F32(unsafe { transmute(value[0]) })
            },
            Type::Scalar(ScalarType::Unsigned(8)) if value.len() == 2 => {
                ConstantValue::U64(unsafe { transmute([value[0], value[1]]) })
            },
            Type::Scalar(ScalarType::Signed(8)) if value.len() == 2 => {
                ConstantValue::I64(unsafe { transmute([value[0], value[1]]) })
            },
            Type::Scalar(ScalarType::Float(8)) if value.len() == 2 => {
                ConstantValue::F64(unsafe { transmute([value[0], value[1]]) })
            },
            _ => return Err(Error::UNSUPPORTED_CONST_TY),
        };
        let constant = ConstantIntermediate {
            value,
            spec_id,
        };
        self.put_const(const_id, constant)
    }
    /// Get the human-friendly name of an instruction result.
    pub fn get_name(&self, id: InstrId) -> Option<&'a str> {
        self.name_map.get(&(id, None)).copied()
    }
    /// Get the human-friendly name of a member of an instruction result.
    pub fn get_member_name(&self, id: InstrId, member_idx: u32) -> Option<&'a str> {
        self.name_map.get(&(id, Some(member_idx))).copied()
    }
    pub fn get_func(&self, func_id: FunctionId) -> Option<&FunctionIntermediate> {
        self.func_map.get(&func_id)
    }
    pub fn get_var_name(&self, locator: Locator) -> Option<&'a str> {
        let instr_id = *self.declr_map.get(&locator)?;
        self.get_name(instr_id)
    }
    fn get_desc_access(&self, var_id: VariableId) -> Option<AccessType> {
        let read_only = self.contains_deco(var_id, None, Decoration::NonWritable);
        let write_only = self.contains_deco(var_id, None, Decoration::NonReadable);
        match (read_only, write_only) {
            (true, true) => None,
            (true, false) => Some(AccessType::ReadOnly),
            (false, true) => Some(AccessType::WriteOnly),
            (false, false) => Some(AccessType::ReadWrite),
        }
    }
    /// Resolve one recurring layer of pointers to the pointer that refer to the
    /// data directly. `ty_id` should be refer to a pointer type. Returns the ID
    /// of the type the pointer points to.
    pub fn access_chain(&self, ty_id: TypeId) -> Option<TypeId> {
        self.ptr_map.get(&ty_id).cloned()
    }
}
impl<'a> ReflectIntermediate<'a> {
    fn populate_entry_points(&mut self, instrs: &'_ mut Peekable<Instrs<'a>>) -> Result<()> {
        while let Some(instr) = instrs.peek() {
            if instr.opcode() != OP_ENTRY_POINT { break; }
            let op = OpEntryPoint::try_from(instr)?;
            let entry_point_declr = EntryPointDeclartion {
                exec_model: op.exec_model,
                func_id: op.func_id,
                name: op.name,
            };
            self.entry_point_declrs.push(entry_point_declr);
            instrs.next();
        }
        Ok(())
    }
    fn populate_execution_modes(&mut self, instrs: &'_ mut Peekable<Instrs<'a>>) -> Result<()> {
        while let Some(instr) = instrs.peek() {
            if instr.opcode() != OP_EXECUTION_MODE { break; }
            let op = OpExecutionMode::try_from(instr)?;
            let execution_mode = match op.execution_mode {
                spirv_headers::ExecutionMode::Invocations => {
                    ExecutionMode::Invocations(op.params[0])
                },
                spirv_headers::ExecutionMode::SpacingEqual => {
                    ExecutionMode::SpacingEqual
                },
                spirv_headers::ExecutionMode::SpacingFractionalEven => {
                    ExecutionMode::SpacingFractionalEven
                },
                spirv_headers::ExecutionMode::SpacingFractionalOdd => {
                    ExecutionMode::SpacingFractionalOdd
                },
                spirv_headers::ExecutionMode::VertexOrderCw => {
                    ExecutionMode::VertexOrderCw
                },
                spirv_headers::ExecutionMode::VertexOrderCcw => {
                    ExecutionMode::VertexOrderCcw
                },
                spirv_headers::ExecutionMode::PixelCenterInteger => {
                    ExecutionMode::PixelCenterInteger
                },
                spirv_headers::ExecutionMode::OriginUpperLeft => {
                    ExecutionMode::OriginUpperLeft
                },
                spirv_headers::ExecutionMode::OriginLowerLeft => {
                    ExecutionMode::OriginLowerLeft
                },
                spirv_headers::ExecutionMode::EarlyFragmentTests => {
                    ExecutionMode::EarlyFragmentTests
                },
                spirv_headers::ExecutionMode::PointMode => {
                    ExecutionMode::PointMode
                },
                spirv_headers::ExecutionMode::Xfb => {
                    ExecutionMode::Xfb
                },
                spirv_headers::ExecutionMode::DepthReplacing => {
                    ExecutionMode::DepthReplacing
                },
                spirv_headers::ExecutionMode::DepthGreater => {
                    ExecutionMode::DepthGreater
                },
                spirv_headers::ExecutionMode::DepthLess => {
                    ExecutionMode::DepthLess
                },
                spirv_headers::ExecutionMode::DepthUnchanged => {
                    ExecutionMode::DepthUnchanged
                },
                spirv_headers::ExecutionMode::LocalSize => {
                    ExecutionMode::LocalSize {
                        x: op.params[0],
                        y: op.params[1],
                        z: op.params[2]
                    }
                },
                spirv_headers::ExecutionMode::InputPoints => {
                    ExecutionMode::InputPoints
                },
                spirv_headers::ExecutionMode::InputLines => {
                    ExecutionMode::InputLines
                },
                spirv_headers::ExecutionMode::InputLinesAdjacency => {
                    ExecutionMode::InputLinesAdjacency
                },
                spirv_headers::ExecutionMode::Triangles => {
                    ExecutionMode::Triangles
                },
                spirv_headers::ExecutionMode::InputTrianglesAdjacency => {
                    ExecutionMode::InputTrianglesAdjacency
                },
                spirv_headers::ExecutionMode::Quads => {
                    ExecutionMode::Quads
                },
                spirv_headers::ExecutionMode::Isolines => {
                    ExecutionMode::Isolines
                },
                spirv_headers::ExecutionMode::OutputVertices => {
                    ExecutionMode::OutputVertices(op.params[0])
                },
                spirv_headers::ExecutionMode::OutputPoints => {
                    ExecutionMode::OutputPoints
                },
                spirv_headers::ExecutionMode::OutputLineStrip => {
                    ExecutionMode::OutputLineStrip
                },
                spirv_headers::ExecutionMode::OutputTriangleStrip => {
                    ExecutionMode::OutputTriangleStrip
                },
                spirv_headers::ExecutionMode::Initializer => {
                    ExecutionMode::Initializer
                },
                spirv_headers::ExecutionMode::Finalizer => {
                    ExecutionMode::Finalizer
                },
                spirv_headers::ExecutionMode::SubgroupSize => {
                    ExecutionMode::SubgroupSize(op.params[0])
                },
                spirv_headers::ExecutionMode::SubgroupsPerWorkgroup => {
                    ExecutionMode::SubgroupsPerWorkgroup(op.params[0])
                },
                spirv_headers::ExecutionMode::SubgroupsPerWorkgroupId => {
                    ExecutionMode::SubgroupsPerWorkgroupId(op.params[0])
                },
                spirv_headers::ExecutionMode::LocalSizeId => {
                    ExecutionMode::LocalSizeId {
                        x: op.params[0],
                        y: op.params[1],
                        z: op.params[2]
                    }
                },
                spirv_headers::ExecutionMode::PostDepthCoverage => {
                    ExecutionMode::PostDepthCoverage
                },
                spirv_headers::ExecutionMode::StencilRefReplacingEXT => {
                    ExecutionMode::StencilRefReplacingEXT
                },
                _ => { return Err(Error::UNSUPPORTED_EXEC_MODE); }
            };
            let execution_mode_declr = ExecutionModeDeclaration {
                func_id: op.func_id,
                execution_mode
            };
            self.execution_mode_declrs.push(execution_mode_declr);
            instrs.next();
        }
        Ok(())
    }
    fn populate_names(&mut self, instrs: &'_ mut Peekable<Instrs<'a>>) -> Result<()> {
        // Extract naming. Names are generally produced as debug information by
        // `glslValidator` but it might be in absence.
        while let Some(instr) = instrs.peek() {
            let (key, value) = match instr.opcode() {
                OP_NAME => {
                    let op = OpName::try_from(instr)?;
                    ((op.target_id, None), op.name)
                },
                OP_MEMBER_NAME => {
                    let op = OpMemberName::try_from(instr)?;
                    ((op.target_id, Some(op.member_idx)), op.name)
                },
                _ => break,
            };
            if !value.is_empty() {
                let collision = self.name_map.insert(key, value);
                if collision.is_some() { return Err(Error::NAME_COLLISION); }
            }
            instrs.next();
        }
        Ok(())
    }
    fn populate_decos(&mut self, instrs: &'_ mut Peekable<Instrs<'a>>) -> Result<()> {
        while let Some(instr) = instrs.peek() {
            let (key, value) = match instr.opcode() {
                OP_DECORATE => {
                    let op = OpDecorate::try_from(instr)?;
                    ((op.target_id, None, op.deco), op.params)
                }
                OP_MEMBER_DECORATE => {
                    let op = OpMemberDecorate::try_from(instr)?;
                    ((op.target_id, Some(op.member_idx), op.deco), op.params)
                },
                x => if is_deco_op(x) { instrs.next(); continue } else { break },
            };
            let collision = self.deco_map.insert(key, value);
            if collision.is_some() { return Err(Error::DECO_COLLISION); }
            instrs.next();
        }
        Ok(())
    }
    fn populate_one_ty(&mut self, instr: &Instr<'a>) -> Result<()> {
        match instr.opcode() {
            OP_TYPE_FUNCTION => {
                Ok(())
            },
            OP_TYPE_VOID => {
                let op = OpTypeVoid::try_from(instr)?;
                self.put_ty(op.ty_id, Type::Void())
            },
            OP_TYPE_BOOL => {
                let op = OpTypeBool::try_from(instr)?;
                let scalar_ty = ScalarType::boolean();
                self.put_ty(op.ty_id, Type::Scalar(scalar_ty))
            },
            OP_TYPE_INT => {
                let op = OpTypeInt::try_from(instr)?;
                let scalar_ty = ScalarType::int(op.nbyte >> 3, op.is_signed);
                self.put_ty(op.ty_id, Type::Scalar(scalar_ty))
            },
            OP_TYPE_FLOAT => {
                let op = OpTypeFloat::try_from(instr)?;
                let scalar_ty = ScalarType::float(op.nbyte >> 3);
                self.put_ty(op.ty_id, Type::Scalar(scalar_ty))
            },
            OP_TYPE_VECTOR => {
                let op = OpTypeVector::try_from(instr)?;
                if let Type::Scalar(scalar_ty) = self.get_ty(op.scalar_ty_id)? {
                    let vec_ty = VectorType::new(scalar_ty.clone(), op.nscalar);
                    self.put_ty(op.ty_id, Type::Vector(vec_ty))
                } else {
                    Err(Error::BROKEN_NESTED_TY)
                }
            },
            OP_TYPE_MATRIX => {
                let op = OpTypeMatrix::try_from(instr)?;
                if let Type::Vector(vec_ty) = self.get_ty(op.vec_ty_id)? {
                    let mat_ty = MatrixType::new(vec_ty.clone(), op.nvec);
                    self.put_ty(op.ty_id, Type::Matrix(mat_ty))
                } else {
                    Err(Error::BROKEN_NESTED_TY)
                }
            },
            OP_TYPE_IMAGE => {
                let op = OpTypeImage::try_from(instr)?;
                let scalar_ty = match self.get_ty(op.scalar_ty_id)? {
                    Type::Scalar(scalar_ty) => Some(scalar_ty.clone()),
                    Type::Void() => None,
                    _ => return Err(Error::BROKEN_NESTED_TY),
                };
                let img_ty = if op.dim == Dim::DimSubpassData {
                    let arng = SubpassDataArrangement::from_spv_def(op.is_multisampled)?;
                    let subpass_data_ty = SubpassDataType::new(scalar_ty, arng);
                    Type::SubpassData(subpass_data_ty)
                } else {
                    // Only unit types allowed to be stored in storage images
                    // can have given format.
                    let unit_fmt = ImageUnitFormat::from_spv_def(
                        op.is_sampled, op.is_depth, op.color_fmt)?;
                    let arng = ImageArrangement::from_spv_def(
                        op.dim, op.is_array, op.is_multisampled)?;
                    let img_ty = ImageType::new(scalar_ty, unit_fmt, arng);
                    Type::Image(img_ty)
                };
                self.put_ty(op.ty_id, img_ty)
            },
            OP_TYPE_SAMPLER => {
                let op = OpTypeSampler::try_from(instr)?;
                // Note that SPIR-V doesn't discriminate color and depth/stencil
                // samplers. `sampler` and `samplerShadow` means the same thing.
                self.put_ty(op.ty_id, Type::Sampler())
            },
            OP_TYPE_SAMPLED_IMAGE => {
                let op = OpTypeSampledImage::try_from(instr)?;
                if let Type::Image(img_ty) = self.get_ty(op.img_ty_id)? {
                    let sampled_img_ty = SampledImageType::new(img_ty.clone());
                    self.put_ty(op.ty_id, Type::SampledImage(sampled_img_ty))
                } else {
                    Err(Error::BROKEN_NESTED_TY)
                }
            },
            OP_TYPE_ARRAY => {
                let op = OpTypeArray::try_from(instr)?;
                let proto_ty = if let Ok(x) = self.get_ty(op.proto_ty_id) { x } else { return Ok(()); };

                let nrepeat = self.get_const(op.nrepeat_const_id)?
                    // Some notes about specialization constants.
                    //
                    // Using specialization constants for array sizes might lead
                    // to UNDEFINED BEHAVIOR because structure size MUST be
                    // definitive at compile time and CANNOT be specialized at
                    // runtime according to Khronos members, but the default
                    // behavior of `glslang` is to treat the specialization
                    // constants as normal constants, then I would say...
                    // probably it's fine to size array with them?
                    .value
                    .to_u32()?;
                let stride = self.get_deco_u32(op.ty_id, Decoration::ArrayStride)
                    .map(|x| x as usize);

                let arr_ty = if let Some(stride) = stride {
                    ArrayType::new(&proto_ty, nrepeat, stride)
                } else {
                    ArrayType::new_multibind(&proto_ty, nrepeat)
                };
                self.put_ty(op.ty_id, Type::Array(arr_ty))
            },
            OP_TYPE_RUNTIME_ARRAY => {
                let op = OpTypeRuntimeArray::try_from(instr)?;
                let proto_ty = if let Ok(x) = self.get_ty(op.proto_ty_id) { x } else { return Ok(()); };
                let stride = self.get_deco_u32(op.ty_id, Decoration::ArrayStride)
                    .map(|x| x as usize);
                let arr_ty = if let Some(stride) = stride {
                    ArrayType::new_unsized(&proto_ty, stride)
                } else {
                    ArrayType::new_unsized_multibind(&proto_ty)
                };
                self.put_ty(op.ty_id, Type::Array(arr_ty))
            },
            OP_TYPE_STRUCT => {
                let op = OpTypeStruct::try_from(instr)?;
                let struct_name = self.get_name(op.ty_id).map(|n| n.to_string());
                let mut struct_ty = StructType::new(struct_name);
                for (i, &member_ty_id) in op.member_ty_ids.iter().enumerate() {
                    let i = i as u32;
                    let mut member_ty = if let Ok(member_ty) = self.get_ty(member_ty_id) {
                        member_ty.clone()
                    } else {
                        return Ok(());
                    };
                    let mut proto_ty = &mut member_ty;
                    while let Type::Array(arr_ty) = proto_ty {
                        proto_ty = &mut *arr_ty.proto_ty;
                    }
                    if let Type::Matrix(ref mut mat_ty) = proto_ty {
                        let mat_stride = self
                            .get_member_deco_u32(op.ty_id, i, Decoration::MatrixStride)
                            .map(|x| x as usize)
                            .ok_or(Error::MISSING_DECO)?;
                        let row_major = self.contains_deco(op.ty_id, Some(i), Decoration::RowMajor);
                        let col_major = self.contains_deco(op.ty_id, Some(i), Decoration::ColMajor);
                        let major = match (row_major, col_major) {
                            (true, false) => MatrixAxisOrder::RowMajor,
                            (false, true) => MatrixAxisOrder::ColumnMajor,
                            _ => return Err(Error::UNENCODED_ENUM),
                        };
                        mat_ty.decorate(mat_stride, major);
                    }
                    let name = if let Some(nm) = self.get_member_name(op.ty_id, i) {
                        if nm.is_empty() { None } else { Some(nm.to_owned()) }
                    } else { None };
                    if let Some(offset) = self.get_member_deco_u32(op.ty_id, i, Decoration::Offset)
                        .map(|x| x as usize) {
                        let member = StructMember {
                            name,
                            offset,
                            ty: member_ty.clone()
                        };
                        struct_ty.members.push(member);
                    } else {
                        // For shader input/output blocks there are no offset
                        // decoration. Since these variables are not externally
                        // accessible we don't have to worry about them.
                        return Ok(())
                    }
                }
                // Don't have to shrink-to-fit because the types in `ty_map`
                // won't be used directly and will be cloned later.
                self.put_ty(op.ty_id, Type::Struct(struct_ty))
            },
            OP_TYPE_POINTER => {
                let op = OpTypePointer::try_from(instr)?;
                if self.ptr_map.insert(op.ty_id, op.target_ty_id).is_some() {
                    return Err(Error::ID_COLLISION)
                } else { return Ok(()) }
            },
            OP_TYPE_ACCELERATION_STRUCTURE_KHR => {
                let op = OpTypeAccelerationStructureKHR::try_from(instr)?;
                self.put_ty(op.ty_id, Type::AccelStruct())
            },
            _ => return Err(Error::UNSUPPORTED_TY),
        }
    }
    fn populate_one_const(&mut self, instr: &Instr<'a>) -> Result<()> {
        let op = OpConstantScalarCommonSPQ::try_from(instr)?;
        match instr.opcode() {
            OP_CONSTANT_TRUE => self.put_bool_const(op.const_id, true, None),
            OP_CONSTANT_FALSE => self.put_bool_const(op.const_id, false, None),
            OP_CONSTANT => self.put_lit_const(op.const_id, op.ty_id, op.value, None),
            _ => Ok(()),
        }
    }
    fn populate_one_spec_const_op(&mut self, instr: &Instr<'a>) -> Result<()> {
        let op = OpSpecConstantHeadSPQ::try_from(instr)?;
        match op.opcode {
            OP_SNEGATE => {
                let op = OpSpecConstantUnaryOpCommonSPQ::try_from(instr)?;
                let a = self.get_const(op.a_id)?.value.to_s32()?;
                let constant = ConstantIntermediate {
                    value: a.overflowing_neg().0.into(),
                    spec_id: None,
                };
                self.put_const(op.spec_const_id, constant)
            },
            OP_NOT => {
                let op = OpSpecConstantUnaryOpCommonSPQ::try_from(instr)?;
                let a = self.get_const(op.a_id)?.value.to_u32()?;
                let constant = ConstantIntermediate {
                    value: (!a).into(),
                    spec_id: None,
                };
                self.put_const(op.spec_const_id, constant)
            },
            OP_IADD => {
                let op = OpSpecConstantBinaryOpCommonSPQ::try_from(instr)?;
                let a = self.get_const(op.a_id)?.value.to_u32()?;
                let b = self.get_const(op.b_id)?.value.to_u32()?;
                let constant = ConstantIntermediate {
                    value: a.overflowing_add(b).0.into(),
                    spec_id: None,
                };
                self.put_const(op.spec_const_id, constant)
            },
            OP_ISUB => {
                let op = OpSpecConstantBinaryOpCommonSPQ::try_from(instr)?;
                let a = self.get_const(op.a_id)?.value.to_u32()?;
                let b = self.get_const(op.b_id)?.value.to_u32()?;
                let constant = ConstantIntermediate {
                    value: a.overflowing_sub(b).0.into(),
                    spec_id: None,
                };
                self.put_const(op.spec_const_id, constant)
            },
            OP_IMUL => {
                let op = OpSpecConstantBinaryOpCommonSPQ::try_from(instr)?;
                let a = self.get_const(op.a_id)?.value.to_u32()?;
                let b = self.get_const(op.b_id)?.value.to_u32()?;
                let constant = ConstantIntermediate {
                    value: a.overflowing_mul(b).0.into(),
                    spec_id: None,
                };
                self.put_const(op.spec_const_id, constant)
            },
            OP_UDIV => {
                let op = OpSpecConstantBinaryOpCommonSPQ::try_from(instr)?;
                let a = self.get_const(op.a_id)?.value.to_u32()?;
                let b = self.get_const(op.b_id)?.value.to_u32()?;
                let constant = ConstantIntermediate {
                    value: a.overflowing_div(b).0.into(),
                    spec_id: None,
                };
                self.put_const(op.spec_const_id, constant)
            },
            OP_SDIV => {
                let op = OpSpecConstantBinaryOpCommonSPQ::try_from(instr)?;
                let a = self.get_const(op.a_id)?.value.to_s32()?;
                let b = self.get_const(op.b_id)?.value.to_s32()?;
                let constant = ConstantIntermediate {
                    value: a.overflowing_div(b).0.into(),
                    spec_id: None,
                };
                self.put_const(op.spec_const_id, constant)
            },
            OP_UMOD => {
                let op = OpSpecConstantBinaryOpCommonSPQ::try_from(instr)?;
                let a = self.get_const(op.a_id)?.value.to_u32()?;
                let b = self.get_const(op.b_id)?.value.to_u32()?;
                let constant = ConstantIntermediate {
                    value: a.overflowing_rem_euclid(b).0.into(),
                    spec_id: None,
                };
                self.put_const(op.spec_const_id, constant)
            },
            OP_SREM => {
                let op = OpSpecConstantBinaryOpCommonSPQ::try_from(instr)?;
                let a = self.get_const(op.a_id)?.value.to_s32()?;
                let b = self.get_const(op.b_id)?.value.to_s32()?;
                let constant = ConstantIntermediate {
                    value: a.overflowing_rem(b).0.into(),
                    spec_id: None,
                };
                self.put_const(op.spec_const_id, constant)
            },
            OP_SMOD => {
                let op = OpSpecConstantBinaryOpCommonSPQ::try_from(instr)?;
                let a = self.get_const(op.a_id)?.value.to_s32()?;
                let b = self.get_const(op.b_id)?.value.to_s32()?;
                let constant = ConstantIntermediate {
                    value: a.overflowing_rem_euclid(b).0.into(),
                    spec_id: None,
                };
                self.put_const(op.spec_const_id, constant)
            },
            OP_SHIFT_RIGHT_LOGICAL => {
                let op = OpSpecConstantBinaryOpCommonSPQ::try_from(instr)?;
                let a = self.get_const(op.a_id)?.value.to_u32()?;
                let b = self.get_const(op.b_id)?.value.to_u32()?;
                let constant = ConstantIntermediate {
                    value: a.overflowing_shr(b).0.into(),
                    spec_id: None,
                };
                self.put_const(op.spec_const_id, constant)
            },
            // Rust don't have a arithmetic shift.
            //OP_SHIFT_RIGHT_ARITHMETIC => {}
            OP_SHIFT_LEFT_LOGICAL => {
                let op = OpSpecConstantBinaryOpCommonSPQ::try_from(instr)?;
                let a = self.get_const(op.a_id)?.value.to_u32()?;
                let b = self.get_const(op.b_id)?.value.to_u32()?;
                let constant = ConstantIntermediate {
                    value: a.overflowing_shl(b).0.into(),
                    spec_id: None,
                };
                self.put_const(op.spec_const_id, constant)
            },
            OP_BITWISE_OR => {
                let op = OpSpecConstantBinaryOpCommonSPQ::try_from(instr)?;
                let a = self.get_const(op.a_id)?.value.to_u32()?;
                let b = self.get_const(op.b_id)?.value.to_u32()?;
                let constant = ConstantIntermediate {
                    value: (a | b).into(),
                    spec_id: None,
                };
                self.put_const(op.spec_const_id, constant)
            },
            OP_BITWISE_XOR => {
                let op = OpSpecConstantBinaryOpCommonSPQ::try_from(instr)?;
                let a = self.get_const(op.a_id)?.value.to_u32()?;
                let b = self.get_const(op.b_id)?.value.to_u32()?;
                let constant = ConstantIntermediate {
                    value: (a ^ b).into(),
                    spec_id: None,
                };
                self.put_const(op.spec_const_id, constant)
            },
            OP_BITWISE_AND => {
                let op = OpSpecConstantBinaryOpCommonSPQ::try_from(instr)?;
                let a = self.get_const(op.a_id)?.value.to_u32()?;
                let b = self.get_const(op.b_id)?.value.to_u32()?;
                let constant = ConstantIntermediate {
                    value: (a & b).into(),
                    spec_id: None,
                };
                self.put_const(op.spec_const_id, constant)
            },
            _ => return Err(Error::UNSUPPORTED_SPEC),
        }
    }
    fn populate_one_spec_const(&mut self, instr: &Instr<'a>, cfg: &ReflectConfig) -> Result<()> {
        match instr.opcode() {
            OP_SPEC_CONSTANT_TRUE | OP_SPEC_CONSTANT_FALSE | OP_SPEC_CONSTANT => {
                let op = OpConstantScalarCommonSPQ::try_from(instr)?;
                let spec_id = self.get_deco_u32(op.const_id, Decoration::SpecId)
                    .ok_or(Error::MISSING_DECO)?;

                if let Some(x) = cfg.spec_values.get(&spec_id) {
                    let constant = ConstantIntermediate {
                        value: x.clone(),
                        spec_id: None,
                    };
                    self.put_const(op.const_id, constant)
                } else {
                    match instr.opcode() {
                        OP_SPEC_CONSTANT_TRUE => self.put_bool_const(op.const_id, true, Some(spec_id)),
                        OP_SPEC_CONSTANT_FALSE => self.put_bool_const(op.const_id, false, Some(spec_id)),
                        OP_SPEC_CONSTANT => self.put_lit_const(op.const_id, op.ty_id, op.value, Some(spec_id)),
                        _ => unreachable!(),
                    }
                }
            },
            // `SpecId` decorations will be specified to each of the
            // constituents so we don't have to register a
            // `SpecConstantIntermediate` for the composite of them.
            // `SpecConstantIntermediate` is registered only for those will be
            // interacting with Vulkan.
            OP_SPEC_CONSTANT_COMPOSITE => {
                //let op = OpSpecConstantComposite::try_from(instr)?;
                //let constant = ConstantIntermediate {
                //    // Empty value to annotate a specialization constant. We
                //    // have nothing like a `SpecId` to access such
                //    // specialization constant so it's unnecesary to resolve
                //    // it's default value. Same applies to `OpSpecConstantOp`.
                //    value: &[] as &'static [u32],
                //    spec_id: None,
                //};
                //(op.spec_const_id, constant)
                return Ok(());
            },
            // Similar to `OpConstantComposite`, we don't register
            // specialization constants for `OpSpecConstantOp` results, neither
            // the validity of the operations because they are out of SPIR-Q's
            // duty.
            //
            // NOTE: In some cases you might want to use specialized workgroup
            // size to allocate shared memory or other on-chip memory with this,
            // that's possible, but still be aware that specialization constants
            // CANNOT be used to specify any STRUCTURED memory objects like UBO
            // and SSBO, because the stride and offset decorations are
            // precompiled as a part of the SPIR-V binary meta.
            OP_SPEC_CONSTANT_OP => self.populate_one_spec_const_op(instr),
            _ => return Err(Error::UNSUPPORTED_SPEC),
        }
    }
    fn populate_one_var(&mut self, instr: &Instr<'a>) -> Result<()> {
        fn extract_proto_ty<'a>(ty: &Type) -> Result<(u32, Type)> {
            match ty {
                Type::Array(arr_ty) => {
                    // `nrepeat=None` is no longer considered invalid because of
                    // the adoption of `SPV_EXT_descriptor_indexing`. This
                    // shader extension has been supported in Vulkan 1.2.
                    let nrepeat = arr_ty.nrepeat()
                        .unwrap_or(0);
                    let proto_ty = arr_ty.proto_ty();
                    Ok((nrepeat, proto_ty.clone()))
                },
                _ => Ok((1, ty.clone())),
            }
        }

        let op = OpVariable::try_from(instr)?;
        let ty_id = self.access_chain(op.ty_id)
            .ok_or(Error::BROKEN_ACCESS_CHAIN)?;
        let ty = if let Ok(ty) = self.get_ty(ty_id) {
            ty
        } else {
            // If a variable is declared based on a unregistered type, very
            // likely it's a input/output block passed between shader stages. We
            // can safely ignore them.
            return Ok(());
        };
        let name = self.get_name(op.var_id).map(|x| x.to_owned());
        let var = match op.store_cls {
            StorageClass::Input => {
                if let Some(location) = self.get_var_location(op.var_id) {
                    let var = Variable::Input { name, location, ty: ty.clone() };
                    // There can be interface blocks for input and output but
                    // there won't be any for attribute inputs nor for
                    // attachment outputs, so we just ignore structs and arrays
                    // or something else here.
                    Some(var)
                } else {
                    // Ignore built-in interface varaibles whichh have no
                    // location assigned.
                    None
                }
            },
            StorageClass::Output => {
                if let Some(location) = self.get_var_location(op.var_id) {
                    let var = Variable::Output { name, location, ty: ty.clone() };
                    Some(var)
                } else {
                    None
                }
            },
            StorageClass::PushConstant => {
                // Push constants have no global offset. Offsets are applied to
                // members.
                if let Type::Struct(_) = ty {
                    let var = Variable::PushConstant { name, ty: ty.clone() };
                    Some(var)
                } else {
                    return Err(Error::TY_NOT_FOUND);
                }
            },
            StorageClass::Uniform => {
                let (nbind, ty) = extract_proto_ty(ty)?;
                let desc_bind = self.get_var_desc_bind_or_default(op.var_id);
                let var = if self.contains_deco(ty_id, None, Decoration::BufferBlock) {
                    let access = self.get_desc_access(op.var_id)
                        .ok_or(Error::ACCESS_CONFLICT)?;
                    let desc_ty = DescriptorType::StorageBuffer(access);
                    Variable::Descriptor { name, desc_bind, desc_ty, ty: ty.clone(), nbind }
                } else {
                    let desc_ty = DescriptorType::UniformBuffer();
                    Variable::Descriptor { name, desc_bind, desc_ty, ty: ty.clone(), nbind }
                };
                Some(var)
            },
            StorageClass::StorageBuffer => {
                let (nbind, ty) = extract_proto_ty(ty)?;
                let desc_bind = self.get_var_desc_bind_or_default(op.var_id);
                let access = self.get_desc_access(op.var_id)
                    .ok_or(Error::ACCESS_CONFLICT)?;
                let desc_ty = DescriptorType::StorageBuffer(access);
                let var = Variable::Descriptor { name, desc_bind, desc_ty, ty: ty.clone(), nbind };
                Some(var)
            },
            StorageClass::UniformConstant => {
                let (nbind, ty) = extract_proto_ty(ty)?;
                let desc_bind = self.get_var_desc_bind_or_default(op.var_id);
                let var = match &ty {
                    Type::Image(img_ty) => {
                        let desc_ty = match img_ty.unit_fmt {
                            ImageUnitFormat::Color(_) => {
                                let access = self.get_desc_access(op.var_id)
                                    .ok_or(Error::ACCESS_CONFLICT)?;
                                match img_ty.arng {
                                    ImageArrangement::ImageBuffer => DescriptorType::StorageTexelBuffer(access),
                                    _ => DescriptorType::StorageImage(access),
                                }
                            },
                            ImageUnitFormat::Sampled => match img_ty.arng {
                                ImageArrangement::ImageBuffer => DescriptorType::UniformTexelBuffer(),
                                _ => DescriptorType::SampledImage(),
                            },
                            ImageUnitFormat::Depth => DescriptorType::SampledImage(),
                        };
                        Variable::Descriptor { name, desc_bind, desc_ty, ty: ty.clone(), nbind }
                    },
                    Type::Sampler() => {
                        let desc_ty = DescriptorType::Sampler();
                        Variable::Descriptor { name, desc_bind, desc_ty, ty: ty.clone(), nbind }
                    },
                    Type::SampledImage(_) => {
                        let desc_ty = if let Type::SampledImage(sampled_img_ty) = &ty {
                            if sampled_img_ty.img_ty.arng == ImageArrangement::ImageBuffer {
                                DescriptorType::UniformTexelBuffer()
                            } else {
                                DescriptorType::CombinedImageSampler()
                            }
                        } else { unreachable!(); };
                        Variable::Descriptor { name, desc_bind, desc_ty, ty: ty.clone(), nbind }
                    },
                    Type::SubpassData(_) => {
                        let input_attm_idx = self
                            .get_deco_u32(op.var_id, Decoration::InputAttachmentIndex)
                            .ok_or(Error::MISSING_DECO)?;
                        let desc_ty = DescriptorType::InputAttachment(input_attm_idx);
                        Variable::Descriptor { name, desc_bind, desc_ty, ty: ty.clone(), nbind }
                    },
                    Type::AccelStruct() => {
                        let desc_ty = DescriptorType::AccelStruct();
                        Variable::Descriptor { name, desc_bind, desc_ty, ty: ty.clone(), nbind }
                    },
                    _ => return Err(Error::UNSUPPORTED_TY),
                };
                Some(var)
            },
            _ => {
                // Leak out unknown storage classes.
                None
            },
        };
        
        if let Some(var) = var {
            // Register variable.
            if self.var_map.insert(op.var_id, self.vars.len()).is_some() {
                return Err(Error::ID_COLLISION);
            }
            let locator = var.locator();
            self.declr_map.insert(locator, op.var_id);
            self.vars.push(var);
        }


        Ok(())
    }
    fn populate_defs(&mut self, instrs: &'_ mut Peekable<Instrs<'a>>, cfg: &ReflectConfig) -> Result<()> {
        // type definitions always follow decorations, so we don't skip
        // instructions here.
        while let Some(instr) = instrs.peek() {
            let opcode = instr.opcode();
            if TYPE_RANGE.contains(&opcode) || opcode == OP_TYPE_ACCELERATION_STRUCTURE_KHR {
                self.populate_one_ty(instr)?;
            } else if opcode == OP_VARIABLE {
                self.populate_one_var(instr)?;
            } else if CONST_RANGE.contains(&opcode) {
                self.populate_one_const(instr)?;
            } else if SPEC_CONST_RANGE.contains(&opcode) {
                self.populate_one_spec_const(instr, cfg)?;
            } else { break; }
            instrs.next();
        }
        Ok(())
    }
    fn populate_access<I: Inspector>(
        &mut self,
        instrs: &'_ mut Peekable<Instrs<'a>>,
        mut inspector: I
    ) -> Result<()> {
        let mut access_chain_map = HashMap::default();
        let mut func_id: InstrId = !0;

        while let Some(instr) = instrs.peek() {
            let mut notify_inspector = func_id != !0;
            // Do our works first.
            match instr.opcode() {
                OP_FUNCTION => {
                    let op = OpFunction::try_from(instr)?;
                    func_id = op.func_id;
                    let last = self.func_map.insert(func_id, Default::default());
                    if last.is_some() {
                        return Err(Error::ID_COLLISION);
                    }
                    notify_inspector = true;
                },
                OP_FUNCTION_CALL => {
                    let op = OpFunctionCall::try_from(instr)?;
                    let func = self.func_map.get_mut(&func_id)
                        .ok_or(Error::FUNC_NOT_FOUND)?;
                    func.callees.insert(op.func_id);
                },
                OP_LOAD | OP_ATOMIC_LOAD |  OP_ATOMIC_EXCHANGE..=OP_ATOMIC_XOR => {
                    let op = OpLoad::try_from(instr)?;
                    let mut var_id = op.var_id;
                    // Resolve access chain.
                    if let Some(&x) = access_chain_map.get(&var_id) { var_id = x }
                    let func = self.func_map.get_mut(&func_id)
                        .ok_or(Error::FUNC_NOT_FOUND)?;
                    func.accessed_vars.insert(var_id);
                },
                OP_STORE | OP_ATOMIC_STORE => {
                    let op = OpStore::try_from(instr)?;
                    let mut var_id = op.var_id;
                    // Resolve access chain.
                    if let Some(&x) = access_chain_map.get(&var_id) { var_id = x }
                    let func = self.func_map.get_mut(&func_id)
                        .ok_or(Error::FUNC_NOT_FOUND)?;
                    func.accessed_vars.insert(var_id);
                },
                OP_ACCESS_CHAIN => {
                    let op = OpAccessChain::try_from(instr)?;
                    if access_chain_map.insert(op.var_id, op.accessed_var_id).is_some() {
                        return Err(Error::ID_COLLISION);
                    }
                },
                OP_FUNCTION_END => {
                    func_id = !0;
                },
                _ => { },
            }
            // Then notify the inspector.
            if notify_inspector {
                inspector.inspect(&self, instr)
            }

            instrs.next();
        }
        Ok(())
    }
    pub(crate) fn reflect<I: Inspector>(
        instrs: Instrs<'a>,
        cfg: &ReflectConfig,
        inspector: I
    ) -> Result<Vec<EntryPoint>> {
        fn skip_until_range_inclusive<'a>(
            instrs: &'_ mut Peekable<Instrs<'a>>,
            rng: RangeInclusive<u32>
        ) {
            while let Some(instr) = instrs.peek() {
                if !rng.contains(&instr.opcode()) { instrs.next(); } else { break; }
            }
        }
        fn skip_until<'a>(instrs: &'_ mut Peekable<Instrs<'a>>, pred: fn(u32) -> bool) {
            while let Some(instr) = instrs.peek() {
                if !pred(instr.opcode()) { instrs.next(); } else { break; }
            }
        }
        // Don't change the order. See _2.4 Logical Layout of a Module_ of the
        // SPIR-V specification for more information.
        let mut instrs = instrs.peekable();
        let mut itm = ReflectIntermediate::default();
        skip_until_range_inclusive(&mut instrs, ENTRY_POINT_RANGE);
        itm.populate_entry_points(&mut instrs)?;
        itm.populate_execution_modes(&mut instrs)?;
        skip_until_range_inclusive(&mut instrs, NAME_RANGE);
        itm.populate_names(&mut instrs)?;
        skip_until(&mut instrs, is_deco_op);
        itm.populate_decos(&mut instrs)?;
        itm.populate_defs(&mut instrs, cfg)?;
        itm.populate_access(&mut instrs, inspector)?;
        itm.collect_entry_points(cfg)
    }
}

/// Reflection configuration builder.
#[derive(Default, Clone)]
pub struct ReflectConfig {
    spv: SpirvBinary,
    ref_all_rscs: bool,
    combine_img_samplers: bool,
    spec_values: HashMap<SpecId, ConstantValue>,
}
impl ReflectConfig {
    pub fn new() -> Self { Default::default() }

    /// SPIR-V binary to be reflected.
    pub fn spv<Spv: Into<SpirvBinary>>(&mut self, x: Spv) -> &mut Self {
        self.spv = x.into();
        self
    }
    /// Reference all defined resources even the resource is not used by an
    /// entry point. Otherwise and by default, only the referenced resources are
    /// assigned to entry points.
    ///
    /// Can be faster for modules with only entry point; slower for multiple
    /// entry points.
    pub fn ref_all_rscs(&mut self, x: bool) -> &mut Self {
        self.ref_all_rscs = x;
        self
    }
    /// Combine images and samplers sharing a same binding point to combined
    /// image sampler descriptors.
    ///
    /// Faster when disabled, but useful for modules derived from HLSL.
    pub fn combine_img_samplers(&mut self, x: bool) -> &mut Self {
        self.combine_img_samplers = x;
        self
    }
    /// Use the provided value for specialization constant at `spec_id`.
    pub fn specialize(&mut self, spec_id: SpecId, value: ConstantValue) -> &mut Self {
        self.spec_values.insert(spec_id, value);
        self
    }

    /// Reflect the SPIR-V binary and extract all entry points.
    pub fn reflect(&self) -> Result<Vec<EntryPoint>> {
        let inspector = NopInspector();
        ReflectIntermediate::reflect(Instrs::new(self.spv.words()), self, inspector)
    }
    /// Reflect the SPIR-V binary and extract all entry points with an inspector
    /// for customized reflection subroutines.
    pub fn reflect_inspect<F>(&self, inspector: F) -> Result<Vec<EntryPoint>>
        where F: FnMut(&ReflectIntermediate<'_>, &Instr<'_>)
    {
        let inspector = FnInspector::<F>(inspector);
        ReflectIntermediate::reflect(Instrs::new(self.spv.words()), self, inspector)
    }
}

impl<'a> ReflectIntermediate<'a> {
    fn collect_fn_vars_impl(&self, func: FunctionId, vars: &mut Vec<VariableId>) {
        if let Some(func) = self.get_func(func) {
            vars.extend(func.accessed_vars.iter());
            for call in func.callees.iter() {
                self.collect_fn_vars_impl(*call, vars);
            }
        }
    }
    fn collect_fn_vars(&self, func: FunctionId) -> Vec<VariableId> {
        let mut accessed_vars = Vec::new();
        self.collect_fn_vars_impl(func, &mut accessed_vars);
        accessed_vars
    }
    fn collect_entry_point_vars(&self, func_id: FunctionId) -> Result<Vec<Variable>> {
        let mut vars = Vec::new();
        for accessed_var_id in self.collect_fn_vars(func_id).into_iter().collect::<HashSet<_>>() {
            // Sometimes this process would meet interface variables without
            // locations. These are should built-ins otherwise the SPIR-V is
            // corrupted. Since we assume the SPIR-V is valid and we don't
            // collect built-in variable as useful information, we simply ignore
            // such null-references.
            if let Some(accessed_var) = self.get_var(accessed_var_id) {
                vars.push(accessed_var.clone());
            }
        }
        Ok(vars)
    }
    fn collect_entry_point_specs(&self) -> Result<Vec<Variable>> {
        // TODO: (penguinlion) Report only specialization constants that have
        // been refered to by the specified function. (Do we actually need this?
        // It might not be an optimization in mind of engineering.)
        let mut vars = Vec::new();
        for constant in self.const_map.values() {
            if let Some(spec_id) = constant.spec_id {
                let locator = Locator::SpecConstant(spec_id);
                let name = self.get_var_name(locator);
                let spec = Variable::SpecConstant {
                    name: name.map(|x| x.to_owned()),
                    spec_id: spec_id,
                    ty: constant.value.ty(),
                };
                vars.push(spec);
            }
        }
        Ok(vars)
    }
    fn collect_exec_modes(&self, func_id: FunctionId) -> Vec<ExecutionMode> {
        self.execution_mode_declrs.iter()
            .filter_map(|declaration| {
                if declaration.func_id == func_id {
                    return Some(declaration.execution_mode.clone());
                }
                None
            })
            .collect()
    }
}

/// Merge `DescriptorType::SampledImage` and `DescriptorType::Sampler` if
/// they are bound to a same binding point with a same number of bindings.
fn combine_img_samplers(vars: Vec<Variable>) -> Vec<Variable> {
    let mut samplers = Vec::<Variable>::new();
    let mut imgs = Vec::<Variable>::new();
    let mut out_vars = Vec::<Variable>::new();

    for var in vars {
        if let Variable::Descriptor { desc_ty, .. } = &var {
            match desc_ty {
                DescriptorType::Sampler() => {
                    samplers.push(var);
                    continue;
                },
                DescriptorType::SampledImage() => {
                    imgs.push(var);
                    continue;
                },
                _ => {},
            }
        } 
        out_vars.push(var);
    }

    for sampler_var in samplers {
        let (sampler_desc_bind, sampler_nbind) = {
            if let Variable::Descriptor { desc_bind, nbind, .. } = sampler_var {
                (desc_bind, nbind)
            } else { unreachable!(); }
        };

        let mut combined_imgs = Vec::new();
        imgs = imgs.drain(..)
            .filter_map(|var| {
                let succ =
                    var.locator() == Locator::Descriptor(sampler_desc_bind) &&
                    var.nbind() == Some(sampler_nbind);
                if succ {
                    combined_imgs.push(var);
                    None
                } else {
                    Some(var)
                }
            })
            .collect();

        if combined_imgs.is_empty() {
            // If the sampler can be combined with no texture, just put it
            // back.
            out_vars.push(sampler_var);
        } else {
            // For any texture that can be combined with this sampler,
            // create a new combined image sampler.
            for img_var in combined_imgs {
                if let Variable::Descriptor { name, ty, .. } = img_var {
                    if let Type::Image(img_ty) = ty {
                        let out_var = Variable::Descriptor {
                            name,
                            desc_bind: sampler_desc_bind,
                            desc_ty: DescriptorType::CombinedImageSampler(),
                            ty: Type::SampledImage(SampledImageType::new(img_ty)),
                            nbind: sampler_nbind,
                        };
                        out_vars.push(out_var);
                    } else { unreachable!(); }
                } else { unreachable!(); }
            }
        }
    }

    out_vars.extend(imgs);

    out_vars
}

impl<'a> ReflectIntermediate<'a> {
    pub fn collect_entry_points(&self, cfg: &ReflectConfig) -> Result<Vec<EntryPoint>> {
        let mut entry_points = Vec::with_capacity(self.entry_point_declrs.len());
        for entry_point_declr in self.entry_point_declrs.iter() {
            let mut vars = if cfg.ref_all_rscs {
                self.vars.clone()
            } else {
                self.collect_entry_point_vars(entry_point_declr.func_id)?
            };
            if cfg.combine_img_samplers {
                vars = combine_img_samplers(vars);
            }
            let specs = self.collect_entry_point_specs()?;
            vars.extend(specs);
            let exec_modes = self.collect_exec_modes(entry_point_declr.func_id);
            let entry_point = EntryPoint {
                name: entry_point_declr.name.to_owned(),
                exec_model: entry_point_declr.exec_model,
                vars,
                exec_modes,
            };
            entry_points.push(entry_point);
        }
        Ok(entry_points)
    }
}
