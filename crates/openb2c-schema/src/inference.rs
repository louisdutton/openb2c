use crate::*;
use serde_json::json;

pub fn foreign_keys<'a>(schema: &'a Schema, table: &str) -> Vec<(&'a str, &'a str, &'a str)> {
    schema
        .tables
        .get(table)
        .into_iter()
        .flatten()
        .filter_map(|(f, c)| reference(c.references.as_deref()?).map(|(t, k)| (f.as_str(), t, k)))
        .collect()
}

pub fn fk(schema: &Schema, table: &str, target: &str) -> anyhow::Result<String> {
    let keys = foreign_keys(schema, table)
        .into_iter()
        .filter(|(_, entity, _)| *entity == target)
        .collect::<Vec<_>>();
    anyhow::ensure!(
        keys.len() == 1,
        "{table}: expected exactly one foreign key to {target}; found {}",
        keys.len()
    );
    Ok(keys[0].0.into())
}

pub fn report(schema: &Schema) -> Value {
    let relationships:Vec<_>=schema.tables.keys().flat_map(|table|foreign_keys(schema,table).into_iter().map(move |(field,target,key)|json!({"entity":table,"field":field,"target":target,"key":key,"reason":"Declared foreign key","evidence":[format!("{table}.{field} references {target}({key})")],"metadata":schema.tables[table][field].relationship}))).collect();
    let profiles:Vec<_>=schema.tables.keys().filter_map(|table|{
        let fks=foreign_keys(schema,table);if fks.len()!=1{return None;}let (field,parent,_)=fks[0];let c=&schema.tables[table][field];let unique=c.unique||schema.constraints.get(table).is_some_and(|g|g.unique.values().any(|c|c.fields.len()==1&&c.fields[0].field==field&&c.condition.is_none()&&c.enforcement!=Enforcement::Application));
        (c.required&&unique).then(||json!({"entity":table,"parentEntity":parent,"field":field,"reason":"One required unique foreign key defines a profile extension","evidence":[format!("{table}.{field}: required, unique, references {parent}")]}))
    }).collect();
    let entities:serde_json::Map<String,Value>=schema.tables.iter().map(|(table,cols)|{
        let label=["name","title","email","reference"].into_iter().find(|f|cols.contains_key(*f));
        let temporal:Vec<_>=cols.iter().filter(|(f,c)|!["created_at","updated_at"].contains(&f.as_str())&&(f.ends_with("_at")||["date","time","date-time"].iter().any(|format|c.metadata["format"]==*format))).map(|(f,_)|f).collect();
        let money:Vec<_>=cols.iter().filter(|(f,c)|f.ends_with("_pence")||c.metadata["format"]=="money").map(|(f,_)|f).collect();
        let status:Vec<_>=cols.iter().filter(|(_,c)|!c.constraints.values.is_empty()).map(|(f,_)|f).collect();
        (table.clone(),json!({"label":{"value":label,"reason":"Prefer a stored human-readable label; use temporal fields and foreign keys otherwise","evidence":label.into_iter().map(|f|format!("{table}.{f}")).collect::<Vec<_>>()},"foreignKeys":relationships.iter().filter(|r|r["entity"]==*table).collect::<Vec<_>>(),"inboundReferences":relationships.iter().filter(|r|r["target"]==*table).collect::<Vec<_>>(),"temporalFields":temporal,"moneyFields":money,"statusFields":status,"uniqueFields":cols.iter().filter(|(_,c)|c.pk||c.unique).map(|(f,_)|f).collect::<Vec<_>>()}))
    }).collect();
    json!({"version":1,"summary":{"tables":schema.tables.len(),"relationships":relationships.len(),"profileExtensions":profiles.len()},"organization":schema.organization,"entities":entities,"relationships":relationships,"profileExtensions":profiles,"profiles":profiles,"operations":schema.operations,"capabilities":schema.capabilities,"auth":schema.metadata.get("auth"),"presentation":schema.metadata.get("presentation")})
}
