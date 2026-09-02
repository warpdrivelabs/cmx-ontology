//! 动作模板注册表（内置）：把常见「本体动作 + 副作用联动」封装为可一键实例化的模板。
//!
//! 模板是纯数据（ActionTypeDef 骨架 + 占位参数），前端「从模板新建动作」拉取后填 apiName 即建。
//! 旗舰模板 `consolClose`（期末关账联动）串起两大跨服务副作用：起关账审批流（flowengine `consol_close`）
//! + 计算关账报表（cmx-report `computeReport`），按 orgCode+periodCode 参数化。

use serde_json::{json, Value};

/// 内置动作模板清单（`{key,name,description,action}`）。action 为 ActionTypeDef 骨架（apiName 待前端填）。
pub fn templates() -> Value {
    json!([
        {
            "key": "consolClose",
            "name": "期末关账联动",
            "description": "对某组织 + 期间发起关账审批流（flowengine consol_close）并计算关账报表（cmx-report）。一个动作串起流程与报表两大联动。",
            "tags": ["关账", "flow", "report"],
            "action": {
                "displayName": "期末关账联动",
                "status": "experimental",
                "parameters": [
                    { "name": "orgCode", "required": true },
                    { "name": "periodCode", "required": true }
                ],
                "logic": [],
                "validations": [],
                "sideEffects": [
                    { "kind": "startBusinessProcess", "flowDefKey": "consol_close", "businessKey": "$periodCode", "orgCode": "$orgCode", "periodCode": "$periodCode" },
                    { "kind": "computeReport", "reportCode": "STAT_01_D", "version": "V2", "orgCode": "$orgCode", "periodCode": "$periodCode" }
                ]
            }
        },
        {
            "key": "objectApprove",
            "name": "对象变更起审批",
            "description": "修改对象状态为「已提交」并发起审批流（占位 flowKey / 对象类型待调整）。",
            "tags": ["审批", "flow"],
            "action": {
                "displayName": "对象变更起审批",
                "status": "experimental",
                "parameters": [
                    { "name": "objectId", "required": true }
                ],
                "logic": [
                    { "op": "modifyObject", "objectType": "Order", "pk": "$objectId", "set": { "status": "submitted" } }
                ],
                "validations": [],
                "sideEffects": [
                    { "kind": "startBusinessProcess", "flowDefKey": "approve", "businessKey": "$objectId", "objectId": "$objectId" }
                ]
            }
        }
    ])
}
