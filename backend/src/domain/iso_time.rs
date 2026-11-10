//! ISO civil timestamps without host timezones or I/O.
#[derive(Clone,Copy,Debug)]
pub struct Timestamp { pub seconds:f64, pub aware:bool }
pub fn parse(raw:&str)->Option<Timestamp> {
    let raw=raw.trim();let bytes=raw.as_bytes();
    if bytes.len()<10 || bytes[4]!=b'-' || bytes[7]!=b'-'{return None;}
    let n=|s:&str|if !s.is_empty()&&s.bytes().all(|b|b.is_ascii_digit()){s.parse::<i64>().ok()}else{None};
    let y=n(raw.get(..4)?)?;let m=n(raw.get(5..7)?)?;let d=n(raw.get(8..10)?)?;
    let leap=y%4==0&&(y%100!=0||y%400==0);
    let days=match m{1|3|5|7|8|10|12=>31,4|6|9|11=>30,2=>if leap{29}else{28},_=>return None};
    if !(1..=9999).contains(&y)||d<1||d>days{return None;}
    let adjusted=y-i64::from(m<=2);let era=adjusted.div_euclid(400);let yoe=adjusted-era*400;
    let mp=m+if m>2{-3}else{9};
    let epoch_days=era*146097+yoe*365+yoe/4-yoe/100+(153*mp+2)/5+d-1-719468;
    if raw.len()==10{return Some(Timestamp{seconds:(epoch_days*86400)as f64,aware:false});}
    if !matches!(bytes[10],b'T'|b' '){return None;}
    let time=raw.get(11..)?;
    let zone_at=time.find(['Z','+','-']);
    let (clock,zone)=zone_at.map_or((time,""),|i|(&time[..i],&time[i..]));
    let mut parts=clock.split(':');let h=n(parts.next()?)?;let min=n(parts.next()?)?;
    let seconds=parts.next().unwrap_or("00").replace(',',".");
    if !seconds.bytes().all(|b|b.is_ascii_digit()||b==b'.'){return None;}
    let sec=seconds.parse::<f64>().ok()?;
    if parts.next().is_some()||!(0..24).contains(&h)||!(0..60).contains(&min)||!(0.0..60.0).contains(&sec){return None;}
    let offset=if zone.is_empty()||zone=="Z"{0}else{
        let sign=if zone.starts_with('+'){1}else if zone.starts_with('-'){-1}else{return None;};
        let zone_digits=&zone[1..];
        if !(zone_digits.len()==4||zone_digits.len()==5&&zone_digits.as_bytes()[2]==b':'){return None;}
        let digits=zone_digits.replace(':',"");if digits.len()!=4{return None;}
        let hours=n(digits.get(..2)?)?;let minutes=n(digits.get(2..)?)?;
        if hours>=24||minutes>=60{return None;}sign*(hours*3600+minutes*60)
    };
    Some(Timestamp{seconds:(epoch_days*86400+h*3600+min*60-offset)as f64+sec,aware:!zone.is_empty()})
}
pub fn span(start:&str,end:&str)->Option<f64>{let a=parse(start)?;let b=parse(end)?;(a.aware==b.aware).then_some(b.seconds-a.seconds)}
pub fn newer(incoming:&str,current:Option<&str>)->bool {
    let Some(i)=parse(incoming)else{return false;};
    current.and_then(parse).is_none_or(|c|i.aware==c.aware&&i.seconds>c.seconds)
}

#[cfg(test)]mod tests{
    use super::*;
    #[test]fn utc_offsets_leaps_and_fractional_instants(){
        assert_eq!(parse("1970-01-01T00:00:00Z").unwrap().seconds,0.0);
        assert_eq!(parse("2000-02-29T00:00:00+00:00").unwrap().seconds,951782400.0);
        assert_eq!(span("2026-01-01T01:00:00+01:00","2026-01-01T00:00:02.5Z"),Some(2.5));
        assert_eq!(span("2026-01-01T00:00:00","2026-01-01T00:00:02Z"),None);
        assert!(newer("2025-12-31T23:00:01Z",Some("2026-01-01T00:00:00+01:00")));
        for invalid in ["","bogus","1900-02-29","2026-13-01","2026-01-32","2026-01-01T24:00:00Z","2026-01-01T00:60:00Z","2026-01-01T00:00:60Z","2026-01-01T00:00:00+25:00","éééééééééé"]{assert!(parse(invalid).is_none(),"{invalid}");}
    }
}
