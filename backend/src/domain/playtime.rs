//! Suspend-aware session duration and native-ingest validation.
use serde_json::{Value,json};
use crate::domain::iso_time;
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Playtime {
    pub total_seconds: i64,
    pub session_count: i64,
    pub last_start: Option<String>,
    pub monotonic_start: Option<f64>,
    pub last_duration: Option<i64>,
    pub last_played: Option<String>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct PendingSession {
    pub rom_id: i64, pub start: String, pub end: String,
    pub device: String, pub duration_ms: i64, pub attempts: i64,
}

pub fn finish(pt:&mut Playtime,end:&str,mono:f64)->Option<(String,i64)> {
    let start=pt.last_start.as_ref()?;
    let wall=iso_time::span(start,end)?.max(0.0);
    let awake=pt.monotonic_start.map(|s|mono-s).filter(|d|*d>=0.0&&*d<=wall+2.0).map(|d|d.min(wall)).unwrap_or(wall);
    let seconds=awake.min(86400.0)as i64;let start=start.clone();
    pt.total_seconds=pt.total_seconds.saturating_add(seconds);pt.session_count=pt.session_count.saturating_add(1);
    pt.last_duration=Some(seconds);pt.last_played=Some(end.into());pt.last_start=None;pt.monotonic_start=None;
    Some((start,seconds))
}
pub fn ingestable(start:&str,end:&str)->bool {
    let (Some(a),Some(b))=(iso_time::parse(start),iso_time::parse(end))else{return false;};
    a.aware==b.aware&&b.seconds.floor()>a.seconds.floor()
}
pub fn rejected_indices(detail:&Value,size:usize)->std::collections::BTreeSet<usize>{
    detail.as_array().into_iter().flatten().filter_map(|row|{
        let loc=row["loc"].as_array()?;
        let i=loc.windows(2).find(|p|p[0]=="sessions")?[1].as_u64()? as usize;
        (i<size).then_some(i)
    }).collect()
}
pub fn projection(p:&Playtime)->Value{json!({"total_seconds":p.total_seconds,"session_count":p.session_count,"last_played":p.last_played})}
pub fn reconcile(p:&mut Playtime,rows:&[Value]) {
    let millis: i64=rows.iter().filter_map(|r|r["duration_ms"].as_f64()).filter(|v|v.is_finite()).map(|v|v as i64).fold(0,i64::saturating_add);
    p.total_seconds=p.total_seconds.max(millis.div_euclid(1000));p.session_count=p.session_count.max(rows.len()as i64);
    for r in rows {if let Some(ts)=r["end_time"].as_str(){if iso_time::newer(ts,p.last_played.as_deref()){p.last_played=Some(ts.into());}}}
}

#[cfg(test)]mod tests{
    use super::*;
    fn open_session()->Playtime{Playtime{last_start:Some("2026-01-01T00:00:00Z".into()),monotonic_start:Some(10.0),..Default::default()}}
    #[test]fn counts_awake_time_with_wall_fallback_clamp_and_clear(){
        for (mono,want) in [(20.9,10),(9.0,60),(100.0,60),(71.0,60)]{
            let mut p=open_session();assert_eq!(finish(&mut p,"2026-01-01T00:01:00Z",mono).unwrap().1,want);
            assert_eq!(p.total_seconds,want);assert_eq!(p.session_count,1);assert!(p.last_start.is_none());assert!(p.monotonic_start.is_none());
        }
        let mut p=open_session();p.monotonic_start=None;assert_eq!(finish(&mut p,"2026-01-03T00:00:00Z",0.0).unwrap().1,86400);
        let mut p=open_session();assert!(finish(&mut p,"bad",20.0).is_none());
        assert_eq!(finish(&mut p,"2025-12-31T00:00:00Z",20.0).unwrap().1,0);
    }
    #[test]fn ingest_checks_second_resolution_and_rejection_indices_are_unambiguous(){
        assert!(!ingestable("2026-01-01T00:00:00.1Z","2026-01-01T00:00:00.9Z"));
        assert!(ingestable("2026-01-01T00:00:00.9Z","2026-01-01T00:00:01Z"));
        assert!(!ingestable("bad","2026-01-01T00:00:01Z"));
        let detail=json!([{"loc":["body","sessions",2,"end_time"]},{"loc":["body","sessions",0]},{"loc":["sessions",true]},{"loc":["sessions",99]},{"loc":["x",1]},null]);
        assert_eq!(rejected_indices(&detail,3),std::collections::BTreeSet::from([0,2]));
        assert!(rejected_indices(&Value::Null,3).is_empty());
    }
    #[test]fn reconcile_ignores_bad_rows_and_never_regresses_local_values(){
        let mut p=Playtime::default();reconcile(&mut p,&[json!({"duration_ms":1000,"end_time":"2026-01-01T00:00:00Z"}),json!({"duration_ms":2001.8,"end_time":"2026-01-01T01:00:01+01:00"}),json!({"duration_ms":true}),json!({"duration_ms":"bad"}),Value::Null]);
        assert_eq!(p.total_seconds,3);assert_eq!(p.session_count,5);assert_eq!(p.last_played.as_deref(),Some("2026-01-01T01:00:01+01:00"));
        reconcile(&mut p,&[json!({"duration_ms":1,"end_time":"2025-01-01T00:00:00Z"})]);assert_eq!(p.total_seconds,3);assert_eq!(p.session_count,5);
    }
}
