use clap::Parser;
use serde_json::json;
use spirq::{
    ty::{StructMember, Type},
    ReflectConfig,
};
use std::path::Path;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg()]
    in_paths: Vec<String>,
    #[arg(
        long,
        help = "Reference all resources even they are never used by the entry \
        points. By default, only the referenced resources are reflected."
    )]
    ref_all_rscs: bool,
}

fn build_spirv_binary<P: AsRef<Path>>(path: P) -> Option<Vec<u8>> {
    use std::fs::File;
    use std::io::Read;
    let path = path.as_ref();
    if !path.is_file() {
        return None;
    }
    let mut buf = Vec::new();
    if let Ok(mut f) = File::open(&path) {
        if buf.len() & 3 != 0 {
            // Misaligned input.
            return None;
        }
        f.read_to_end(&mut buf).ok()?;
    }
    Some(buf)
}

fn member2json(member: &StructMember) -> serde_json::Value {
    json!({
        "Name": member.name,
        "Offset": member.offset,
        "MemberType": ty2json(&member.ty)
    })
}
fn ty2json(ty: &Type) -> serde_json::Value {
    match ty {
        Type::Matrix(x) => json!({
            "Kind": "Matrix",
            "AxisOrder": x.major.map(|x| format!("{:?}", x)),
            "VectorType": x.vec_ty.to_string(),
            "Count": x.nvec,
            "Stride": x.stride,
        }),
        Type::Array(x) => json!({
            "Kind": "Array",
            "ElementType": ty2json(&*x.proto_ty),
            "Count": x.nrepeat,
            "Stride": x.stride
        }),
        Type::Struct(x) => json!({
            "Kind": "Struct",
            "Members": x.members.iter().map(member2json).collect::<Vec<_>>()
        }),
        Type::DevicePointer(x) => json!({
            "Kind": "Pointer",
            "TargetType": ty2json(&*x.pointee_ty)
        }),
        _ => json!(ty.to_string()),
    }
}

fn main() {
    let args = Args::parse();

    for in_path in args.in_paths {
        let spv = build_spirv_binary(&in_path).expect(&format!("cannot read spirv: {}", in_path));
        let entry_points = ReflectConfig::new()
            .spv(spv)
            .ref_all_rscs(args.ref_all_rscs)
            .gen_unique_names(true)
            .reflect()
            .expect(&format!("cannot reflect spirv: {}", in_path));

        for entry_point in entry_points {
            let mut inputs = Vec::new();
            let mut outputs = Vec::new();
            let mut descs = Vec::new();
            let mut push_consts = Vec::new();
            let mut spec_consts = Vec::new();
            for var in entry_point.vars {
                use spirq::Variable::*;
                match var {
                    Input { name, location, ty } => {
                        let j = json!({
                            "Name": name.unwrap(),
                            "Location": location.loc(),
                            "Component": location.comp(),
                            "Type": ty2json(&ty),
                        });
                        inputs.push(j);
                    }
                    Output { name, location, ty } => {
                        let j = json!({
                            "Name": name.unwrap(),
                            "Location": location.loc(),
                            "Component": location.comp(),
                            "Type": ty2json(&ty),
                        });
                        outputs.push(j);
                    }
                    Descriptor {
                        name,
                        desc_bind,
                        desc_ty,
                        ty,
                        nbind,
                    } => {
                        let j = json!({
                            "Name": name.unwrap(),
                            "Set": desc_bind.set(),
                            "Binding": desc_bind.bind(),
                            "DescriptorType": format!("{desc_ty:?}"),
                            "Type": ty2json(&ty),
                            "Count": nbind,
                        });
                        descs.push(j);
                    }
                    PushConstant { name, ty } => {
                        let j = json!({
                            "Name": name.unwrap(),
                            "Type": ty2json(&ty),
                        });
                        push_consts.push(j);
                    }
                    SpecConstant { name, spec_id, ty } => {
                        let j = json!({
                            "Name": name.unwrap(),
                            "SpecId": spec_id,
                            "Type": ty2json(&ty),
                        });
                        spec_consts.push(j);
                    }
                }
            }

            let mut exec_modes = Vec::new();
            for exec_mode in entry_point.exec_modes {
                let operands = exec_mode
                    .operands
                    .iter()
                    .map(|operand| {
                        json!({
                            "Value": operand.value.to_u32(),
                            "SpecId": operand.spec_id,
                        })
                    })
                    .collect::<Vec<_>>();
                let j = json!({
                    "ExecutionMode": format!("{:?}", exec_mode.exec_mode),
                    "Operands": operands,
                });
                exec_modes.push(j);
            }

            let j = json!({
                "EntryPoint": entry_point.name,
                "ExecutionModel": format!("{:?}", entry_point.exec_model),
                "ExecutionModes": exec_modes,
                "Variables": {
                    "Inputs": inputs,
                    "Outputs": outputs,
                    "Descriptors": descs,
                    "PushConstants": push_consts,
                    "SpecConstants": spec_consts
                },
            });

            println!("{}", serde_json::to_string_pretty(&j).unwrap());
        }
    }
}
