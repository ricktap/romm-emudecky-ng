//! Conservative text VDF parser and Steam Input patcher. Unknown keys survive.
#[derive(Clone,Debug,PartialEq,Eq)]
pub enum Node {Text(String),Object(Vec<(String,Node)>)}
fn tokens(input:&str)->Result<Vec<String>,String> {
    let mut it=input.chars().peekable();let mut out=vec![];
    while let Some(c)=it.next(){match c {
        c if c.is_whitespace()=>{},
        '/' if it.peek()==Some(&'/')=>{it.next();for c in it.by_ref(){if c=='\n'{break;}}},
        '{'|'}'=>out.push(c.to_string()),
        '"'=>{let mut s=String::new();let mut ended=false;while let Some(c)=it.next(){if c=='"'{ended=true;break;}if c=='\\'{let next=it.next().ok_or("unfinished escape")?;match next{'"'|'\\'=>s.push(next),'n'=>s.push('\n'),'t'=>s.push('\t'),_=>{s.push('\\');s.push(next);}}}else{s.push(c);}}if !ended{return Err("unfinished string".into());}out.push(format!("s{s}"));},
        _=>{let mut s=String::from(c);while it.peek().is_some_and(|c|!c.is_whitespace()&&!matches!(c,'{'|'}')){s.push(it.next().unwrap());}out.push(format!("s{s}"));}
    }}Ok(out)
}
pub fn parse(input:&str)->Result<Node,String> {
    fn object(t:&[String],i:&mut usize,nested:bool,depth:usize)->Result<Node,String>{
        if depth>128{return Err("VDF nesting too deep".into());}let mut entries=vec![];
        while *i<t.len(){if t[*i]=="}"{if !nested{return Err("unexpected closing brace".into());}*i+=1;return Ok(Node::Object(entries));}
            let key=t[*i].strip_prefix('s').ok_or("expected key")?.to_string();*i+=1;
            let token=t.get(*i).ok_or("missing value")?;*i+=1;
            let value=if token=="{"{object(t,i,true,depth+1)?}else{Node::Text(token.strip_prefix('s').ok_or("expected value")?.into())};entries.push((key,value));
        }if nested{Err("unclosed object".into())}else{Ok(Node::Object(entries))}
    }
    object(&tokens(input)?,&mut 0,false,0)
}
fn quote(s:&str)->String{format!("\"{}\"",s.replace('\\',"\\\\").replace('"',"\\\"").replace('\n',"\\n").replace('\t',"\\t"))}
pub fn render(node:&Node)->String {
    fn write(node:&Node,depth:usize,out:&mut String){if let Node::Object(entries)=node{for (key,value) in entries{out.push_str(&"\t".repeat(depth));out.push_str(&quote(key));match value{Node::Text(text)=>{out.push('\t');out.push_str(&quote(text));out.push('\n');},Node::Object(_)=>{out.push('\n');out.push_str(&"\t".repeat(depth));out.push_str("{\n");write(value,depth+1,out);out.push_str(&"\t".repeat(depth));out.push_str("}\n");}}}}}
    let mut out=String::new();write(node,0,&mut out);out
}
fn object_at<'a>(node:&'a mut Node,path:&[&str],create:bool)->Result<Option<&'a mut Vec<(String,Node)>>,String>{
    let Node::Object(entries)=node else{return Err("expected object".into());};
    if path.is_empty(){return Ok(Some(entries));}
    let index=entries.iter().position(|(k,_)|k==path[0]);
    let index=match index{Some(i)=>i,None if create=>{entries.push((path[0].into(),Node::Object(vec![])));entries.len()-1},None=>return Ok(None)};
    object_at(&mut entries[index].1,&path[1..],create)
}
pub fn patch(node:&mut Node,ids:&[i64],mode:&str)->Result<bool,String>{
    let value=match mode{"force_on"=>Some("2"),"force_off"=>Some("0"),_=>None};
    let Some(apps)=object_at(node,&["UserLocalConfigStore","Apps"],value.is_some())?else{return Ok(false)};let mut changed=false;
    for id in ids{
        let key=id.to_string();let index=apps.iter().position(|(k,_)|k==&key);
        if let Some(value)=value{
            let index=index.unwrap_or_else(||{apps.push((key.clone(),Node::Object(vec![])));apps.len()-1});
            let Node::Object(app)=&mut apps[index].1 else{return Err("app config is not an object".into());};
            if let Some((_,current))=app.iter_mut().find(|(k,_)|k=="UseSteamControllerConfig"){if *current!=Node::Text(value.into()){*current=Node::Text(value.into());changed=true;}}
            else{app.push(("UseSteamControllerConfig".into(),Node::Text(value.into())));changed=true;}
        }else if let Some(index)=index{
            let Node::Object(app)=&mut apps[index].1 else{return Err("app config is not an object".into());};let before=app.len();app.retain(|(k,_)|k!="UseSteamControllerConfig");changed|=before!=app.len();if app.is_empty(){apps.remove(index);}
        }
    }Ok(changed)
}
#[cfg(test)]mod tests{
    use super::*;
    #[test]fn roundtrip_unknown_keys_and_patch_only_target_apps(){let mut node=parse("// comment\n\"UserLocalConfigStore\" { \"Other\" \"日本語\" \"Apps\" { \"123\" { \"Unrelated\" \"x\" } \"456\" { \"UseSteamControllerConfig\" \"2\" } } }").unwrap();assert!(patch(&mut node,&[123],"force_off").unwrap());assert_eq!(parse(&render(&node)).unwrap(),node);assert!(!patch(&mut node,&[123],"force_off").unwrap());assert!(patch(&mut node,&[123],"default").unwrap());assert!(render(&node).contains("Unrelated"));assert!(render(&node).contains("456"));assert!(patch(&mut node,&[789],"force_on").unwrap());assert!(patch(&mut node,&[789],"default").unwrap());assert!(!render(&node).contains("789"));}
    #[test]fn malformed_input_and_scalar_apps_are_never_rewritten(){for bad in ["\"key\"","}","\"a\" {","\"a", "\"a\" }"]{assert!(parse(bad).is_err());}let mut node=parse("\"UserLocalConfigStore\" { \"Apps\" \"bad\" }").unwrap();assert!(patch(&mut node,&[1],"force_on").is_err());}
}
