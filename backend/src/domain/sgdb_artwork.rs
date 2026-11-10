//! SteamGridDB wire projections and endpoint vocabulary.
use serde_json::{Value,json};
pub const ASSETS:[&str;4]=["hero","logo","grid","icon"];
pub fn asset_name(n:i64)->Option<&'static str>{n.checked_sub(1).and_then(|n|usize::try_from(n).ok()).and_then(|n|ASSETS.get(n).copied())}
pub fn endpoint(kind:&str,id:i64)->Option<String>{
    let segment=match kind{"hero"=>"heroes","logo"=>"logos","grid"=>"grids","icon"=>"icons",_=>return None};
    Some(format!("/{segment}/game/{id}{}",if kind=="grid"{"?dimensions=460x215,920x430"}else{""}))
}
pub fn first_image(v:&Value,thumb:bool)->Option<&str>{
    if v["success"]!=true{return None;}
    let first=v["data"].as_array()?.first()?;
    if thumb {if let Some(s)=first["thumb"].as_str().filter(|s|!s.is_empty()){return Some(s);}}
    first["url"].as_str().filter(|s|!s.is_empty())
}
pub fn candidates(v:&Value)->Vec<Value>{
    if v["success"]!=true{return vec![];}
    v["data"].as_array().into_iter().flatten().filter_map(|v|{
        let id=v["id"].as_i64()?;let name=v["name"].as_str()?;
        let year=v["release_date"].as_f64().filter(|t|t.is_finite()&&*t>=-62_135_596_800.0&&*t<253_402_300_800.0).map(|t|{
            // Gregorian civil year from epoch days, including pre-1970 releases.
            let z=(t.floor() as i64).div_euclid(86400)+719468;
            let era=z.div_euclid(146097);let doe=z.rem_euclid(146097);
            let yoe=(doe-doe/1460+doe/36524-doe/146096)/365;
            let doy=doe-(365*yoe+yoe/4-yoe/100);let mp=(5*doy+2)/153;
            yoe+era*400+i64::from(mp>=10)
        });
        Some(json!({"id":id,"name":name,"release_year":year}))
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_codes_and_endpoints_match_frontend_vocabulary() {
        for (code, kind, path) in [
            (1, "hero", "/heroes/game/9"), (2, "logo", "/logos/game/9"),
            (3, "grid", "/grids/game/9?dimensions=460x215,920x430"), (4, "icon", "/icons/game/9"),
        ] {
            assert_eq!(asset_name(code), Some(kind));
            assert_eq!(endpoint(kind, 9).as_deref(), Some(path));
        }
        for code in [i64::MIN, -1, 0, 5, i64::MAX] { assert_eq!(asset_name(code), None); }
        assert_eq!(endpoint("poster", 9), None);
    }

    #[test]
    fn malformed_payloads_degrade_and_thumbnails_prefer_small_images() {
        for v in [Value::Null, json!({}), json!({"success":false,"data":[{"url":"x"}]}),
            json!({"success":true,"data":{}}), json!({"success":true,"data":[null]}),
            json!({"success":true,"data":[]})] {
            assert_eq!(first_image(&v, true), None);
            assert!(candidates(&v).is_empty());
        }
        let v = json!({"success":true,"data":[{"url":"full","thumb":"small"}]});
        assert_eq!(first_image(&v, true), Some("small"));
        assert_eq!(first_image(&v, false), Some("full"));
        assert_eq!(first_image(&json!({"success":true,"data":[{"url":"full","thumb":""}]}), true), Some("full"));
        assert_eq!(first_image(&json!({"success":true,"data":[{"url":""}]}), false), None);
    }

    #[test]
    fn candidates_filter_invalid_rows_and_convert_utc_year_boundaries() {
        let dates = [json!(0), json!(-0.5), json!(946684800), json!(-62135596800i64),
            json!(253402300799i64), json!(253402300800i64), json!(true), json!("2000"), Value::Null];
        let mut rows: Vec<Value> = dates.into_iter().enumerate().map(|(i,date)|
            json!({"id":i,"name":"Game","release_date":date})).collect();
        rows.extend([json!({"id":"1","name":"Bad"}), json!({"id":1,"name":false}), json!(false)]);
        let got = candidates(&json!({"success":true,"data":rows}));
        assert_eq!(got.len(), 9);
        assert_eq!(got.iter().map(|r|r["release_year"].clone()).collect::<Vec<_>>(),
            vec![json!(1970), json!(1969), json!(2000), json!(1), json!(9999), Value::Null, Value::Null, Value::Null, Value::Null]);
    }
}
