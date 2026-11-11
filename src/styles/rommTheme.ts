/**
 * The RomM UI theme: the class names and values of the design prototype (docs/design/ui-redesign-prototype.html),
 * every class prefixed `rm-` so nothing collides with Steam's own CSS. A gamepad focus (Decky adds `gpfocus`) looks
 * like the prototype's hover. Generated from the prototype's stylesheet; edit here from now on.
 */
export const ROMM_CSS = `
.rm-root{background:#10151c;color:#dcdedf}
/* Steam's footer bar covers the bottom ~40 px: whatever takes the focus scrolls to leave room below it. */
.rm-root *{box-sizing:border-box;scroll-margin-top:24px;scroll-margin-bottom:64px}
.rm-lmain,.rm-prail{scroll-padding-top:24px;scroll-padding-bottom:64px}
.rm-root button{font-family:inherit}
.rm-glyph{min-width:22px;height:22px;border-radius:11px;background:#dcdedf;color:#0a0c0f;display:inline-flex;align-items:center;justify-content:center;font-weight:800;font-size:12px;padding:0 6px}
.rm-btn{border:0;font-size:14px;font-weight:500;background:#2b313a;color:#dcdedf;border-radius:3px;padding:0 14px;height:40px;cursor:pointer;display:inline-flex;align-items:center;justify-content:center;gap:8px;white-space:nowrap}
.rm-btn:hover,.rm-btn.gpfocus{background:#dcdedf;color:#0e141b}
.rm-btn.rm-primary{background:#1a9fff;color:#fff}
.rm-btn.rm-primary:hover,.rm-btn.rm-primary.gpfocus{background:#5cbaff;color:#fff}
.rm-btn.rm-play{background:#59bf40;color:#fff}
.rm-btn.rm-play:hover,.rm-btn.rm-play.gpfocus{background:#74d65b;color:#fff}
.rm-btn.rm-warn{background:#d4a72c;color:#1a1408}
.rm-btn.rm-ghost{background:transparent;border:1px solid #3a414c}
.rm-btn.rm-ghost:hover,.rm-btn.rm-ghost.gpfocus{background:#dcdedf;color:#0e141b}
.rm-btn.rm-danger{color:#ff8a8a}
.rm-btn.rm-danger:hover,.rm-btn.rm-danger.gpfocus{background:#d4343c;color:#fff}
.rm-btn.rm-dangerfill{background:#d4343c;color:#fff}
.rm-btn.rm-sm{height:32px;font-size:13px;padding:0 10px}
.rm-btn.rm-block{width:100%}
.rm-btn.rm-icon{width:40px;padding:0}
.rm-btn.rm-sm.rm-icon{width:32px}
.rm-qhead{display:flex;align-items:center;gap:10px;margin-bottom:12px}
.rm-qhead h2{margin:0;font-size:19px;font-weight:700;color:#fff}
.rm-qsec{font-size:11px;font-weight:700;letter-spacing:.09em;text-transform:uppercase;color:#8b929a;margin:18px 0 8px}
.rm-status{display:flex;align-items:center;gap:8px;font-size:13px;color:#a3aab3}
.rm-dot{width:8px;height:8px;border-radius:4px;background:#59bf40;display:inline-block;flex:none}
.rm-dot.rm-amber{background:#d4a72c}
.rm-dot.rm-red{background:#d4343c}
.rm-dot.rm-grey{background:#5c636d}
.rm-card{background:#171c23;border-radius:6px;padding:12px}
.rm-card h3{margin:0 0 4px;font-size:15px;font-weight:600;color:#fff}
.rm-muted{color:#8b929a;font-size:13px}
.rm-row{display:flex;align-items:center;justify-content:space-between;gap:10px;padding:11px 0;border-bottom:1px solid #222831;font-size:14px}
.rm-row .rm-v{color:#8b929a;font-size:13px;text-align:right}
.rm-small{font-size:12px}
.rm-navrow{width:100%;display:flex;align-items:center;gap:10px;background:#171c23;border:0;color:#dcdedf;border-radius:4px;padding:0 12px;height:44px;font-size:14px;cursor:pointer;margin-bottom:4px}
.rm-navrow:hover,.rm-navrow.gpfocus{background:#dcdedf;color:#0e141b}
.rm-navrow .rm-sp{flex:1;text-align:left}
.rm-badge{background:#1a9fff;color:#fff;border-radius:9px;font-size:11px;font-weight:700;padding:2px 7px}
.rm-att{width:100%;display:flex;align-items:center;gap:10px;background:#231f15;border:1px solid #4a3f1d;color:#dcdedf;border-radius:4px;padding:9px 12px;font-size:13px;cursor:pointer;text-align:left;margin-bottom:6px}
.rm-att:hover,.rm-att.gpfocus{background:#dcdedf;color:#0e141b;border-color:#dcdedf}
.rm-att .rm-t{font-weight:600;display:block}
.rm-att .rm-s{font-size:12px;opacity:.75}
.rm-amberc{color:#d4a72c}
.rm-bar{height:6px;background:#2b313a;border-radius:3px;overflow:hidden;display:block}
.rm-bar span{display:block;height:100%;background:#1a9fff}
.rm-dlrow{display:flex;align-items:center;gap:10px;padding:10px 0;border-bottom:1px solid #222831}
.rm-sw{width:44px;height:24px;border-radius:12px;background:#3a414c;border:0;position:relative;cursor:pointer;flex:none}
.rm-sw .rm-knob{position:absolute;top:3px;left:3px;width:18px;height:18px;border-radius:9px;background:#dcdedf;transition:left .15s}
.rm-sw.rm-on{background:#1a9fff}
.rm-sw.rm-on .rm-knob{left:23px}
.rm-thumb{width:36px;height:48px;border-radius:3px;flex:none;display:flex;align-items:flex-end;font-size:9px;font-weight:800;color:rgba(255,255,255,.75);padding:3px}
.rm-lib{height:100%;padding-top:40px;padding-bottom:40px;display:flex;flex-direction:column;background:#10151c;box-sizing:border-box}
.rm-lhead{display:flex;align-items:center;gap:16px;padding:8px 28px 0;height:56px}
.rm-lhead h1{margin:0;font-size:22px;font-weight:800;color:#fff}
.rm-ltabs{display:flex;align-items:center;gap:4px;padding:0 28px;border-bottom:1px solid #222831}
.rm-ltab{border:0;background:transparent;color:#8b929a;font-size:14px;font-weight:600;text-transform:uppercase;letter-spacing:.04em;padding:14px;cursor:pointer;border-bottom:3px solid transparent;display:inline-flex;align-items:center;gap:8px}
.rm-ltab.rm-on{color:#fff;border-bottom-color:#1a9fff}
.rm-ltab:hover,.rm-ltab.gpfocus{color:#fff;background:#1c222b}
.rm-searchwrap{position:relative;margin-left:auto}
.rm-searchwrap svg{position:absolute;left:10px;top:10px;color:#8b929a}
.rm-search{height:36px;border-radius:4px;border:1px solid #2f3540;background:#171c23;color:#fff;padding:0 12px 0 34px;font-size:14px;width:260px;font-family:inherit}
.rm-qam{padding-bottom:48px}
.rm-lbody{flex:1;display:flex;min-height:0}
.rm-prail{width:230px;flex:none;padding:16px 12px;border-right:1px solid #222831;overflow-y:auto}
.rm-pbtn{width:100%;display:flex;align-items:center;justify-content:space-between;border:0;background:transparent;color:#c3c8ce;font-size:14px;padding:0 12px;height:40px;border-radius:4px;cursor:pointer;text-align:left}
.rm-pbtn.rm-on{background:#23303f;color:#fff}
.rm-pbtn:hover,.rm-pbtn.gpfocus{background:#dcdedf;color:#0e141b}
.rm-pbtn .rm-n{font-size:12px;opacity:.7}
.rm-pbtn.rm-off{opacity:.45}
.rm-lmain{flex:1;min-width:0;overflow-y:auto;padding:16px 24px 56px}
.rm-chips{display:flex;gap:8px;margin-bottom:16px;align-items:center}
.rm-fchip{display:inline-flex;align-items:center;border:1px solid #2f3540;background:transparent;color:#c3c8ce;font-size:13px;font-weight:500;padding:0 12px;height:32px;border-radius:16px;cursor:pointer}
.rm-fchip.rm-on{background:#dcdedf;color:#0e141b;border-color:#dcdedf}
.rm-fchip:hover,.rm-fchip.gpfocus{border-color:#fff}
.rm-grid{display:grid;grid-template-columns:repeat(6,minmax(0,1fr));gap:18px}
.rm-gcard{border:0;padding:0;background:none;cursor:pointer;display:flex;flex-direction:column;gap:7px;text-align:left;color:#dcdedf}
.rm-cover{position:relative;display:block;aspect-ratio:2/3;border-radius:4px;overflow:hidden;transition:transform .12s;box-shadow:0 4px 12px rgba(0,0,0,.35)}
.rm-gcard:hover .rm-cover,.rm-gcard.gpfocus .rm-cover{outline:3px solid #fff;transform:scale(1.04)}
.rm-ptag{position:absolute;left:8px;top:8px;font-size:10px;font-weight:800;letter-spacing:.06em;background:rgba(0,0,0,.45);color:#fff;padding:2px 6px;border-radius:3px}
.rm-ctitle{position:absolute;left:10px;right:10px;bottom:12px;font-size:15px;font-weight:800;line-height:1.15;color:#fff}
.rm-sbadge{position:absolute;right:8px;top:8px;width:24px;height:24px;border-radius:12px;display:flex;align-items:center;justify-content:center}
.rm-sb-ok{background:#59bf40;color:#fff}
.rm-sb-cloud{background:rgba(0,0,0,.55);color:#dcdedf}
.rm-sb-warn{background:#d4a72c;color:#1a1408}
.rm-sb-dl{background:#1a9fff;color:#fff}
.rm-cbar{position:absolute;left:0;right:0;bottom:0;height:5px;background:rgba(0,0,0,.5)}
.rm-cbar span{display:block;height:100%;background:#1a9fff}
.rm-gname{font-size:13px;font-weight:600;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
.rm-gsub{font-size:12px;color:#8b929a;margin-top:-4px}
.rm-gsub.rm-warn{color:#d4a72c}
.rm-tbl{width:100%;border-collapse:collapse;font-size:14px}
.rm-tbl th{font-size:11px;font-weight:700;letter-spacing:.08em;text-transform:uppercase;color:#8b929a;text-align:left;padding:8px 10px;border-bottom:1px solid #2a3039}
.rm-tbl td{padding:10px;border-bottom:1px solid #1f252d;vertical-align:middle}
.rm-corebtn{border:1px solid #2f3540;background:#171c23;color:#dcdedf;border-radius:4px;height:32px;padding:0 10px;font-size:13px;cursor:pointer;display:inline-flex;align-items:center;gap:6px}
.rm-corebtn:hover,.rm-corebtn.gpfocus{background:#dcdedf;color:#0e141b}
.rm-setgrid{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:18px;align-items:start}
.rm-col{display:flex;flex-direction:column;gap:18px}
.rm-scard{background:#161b22;border-radius:6px;padding:4px 16px 8px}
.rm-scard h3{margin:14px 0 4px;font-size:12px;font-weight:700;letter-spacing:.09em;text-transform:uppercase;color:#8b929a}
.rm-scard .rm-row:last-child{border:0}
.rm-game{position:absolute;inset:0;background:#141a21;display:flex;flex-direction:column}
.rm-hero{height:250px;flex:none;position:relative;display:flex;align-items:flex-end;padding:0 40px 26px}
.rm-htitle{font-size:46px;font-weight:800;color:#fff;letter-spacing:-.02em;line-height:1.02;max-width:760px}
.rm-hplat{position:absolute;right:40px;bottom:28px;font-size:13px;font-weight:700;letter-spacing:.08em;text-transform:uppercase;color:rgba(255,255,255,.8)}
.rm-abar{display:flex;align-items:center;gap:14px;padding:16px 40px 10px}
.rm-big{height:52px;min-width:230px;font-size:17px;font-weight:600}
.rm-pbig{position:relative;overflow:hidden;background:#24364a;color:#fff}
.rm-pbig .rm-fill{position:absolute;left:0;top:0;bottom:0;background:#1a9fff;opacity:.55}
.rm-pbig .rm-lt{position:relative}
.rm-pick{border:1px solid #333a45;background:#1b2129;color:#dcdedf;border-radius:3px;height:52px;padding:0 12px;font-size:13px;cursor:pointer;display:inline-flex;flex-direction:column;align-items:flex-start;justify-content:center;gap:2px}
.rm-pick:hover,.rm-pick.gpfocus{background:#dcdedf;color:#0e141b}
.rm-pk{font-size:10px;font-weight:700;letter-spacing:.08em;text-transform:uppercase;opacity:.65}
.rm-pv{display:flex;align-items:center;gap:6px;font-weight:600}
.rm-stat{display:flex;flex-direction:column;gap:3px;padding:0 8px}
.rm-sk{font-size:11px;font-weight:700;letter-spacing:.08em;text-transform:uppercase;color:#8b929a}
.rm-sv{font-size:14px}
.rm-ready{display:flex;gap:10px;padding:2px 40px 12px}
.rm-rchip{flex:1;min-width:0;display:flex;align-items:center;gap:10px;border:1px solid #262d37;background:#1a2029;border-radius:6px;padding:9px 12px;height:58px}
.rm-rchip.rm-warn{border-color:#5e4b1b;background:#272114}
.rm-rt{font-size:14px;font-weight:600;color:#fff}
.rm-rs{font-size:12px;color:#8b929a;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
.rm-rtx{flex:1;min-width:0}
.rm-ic-ok{color:#59bf40}
.rm-ic-warn{color:#d4a72c}
.rm-ic-neu{color:#8b929a}
.rm-ic-busy{color:#1a9fff}
.rm-dtabs{display:flex;gap:4px;padding:0 40px;border-bottom:1px solid #232a33}
.rm-dtab{border:0;background:transparent;color:#8b929a;font-size:15px;font-weight:600;text-transform:uppercase;padding:12px 14px;cursor:pointer;border-bottom:3px solid transparent}
.rm-dtab.rm-on{color:#fff;border-bottom-color:#1a9fff}
.rm-dtab:hover,.rm-dtab.gpfocus{color:#fff;background:#1c222b}
.rm-dbody{flex:1;overflow-y:auto;padding:18px 40px 56px}
.rm-kv{display:grid;grid-template-columns:180px 1fr;gap:10px 18px;font-size:14px}
.rm-kv .rm-k{color:#8b929a}
.rm-modal{width:460px;background:#1c2129;border-radius:6px;box-shadow:0 18px 50px rgba(0,0,0,.6);overflow:hidden}
.rm-modal.rm-wide{width:640px}
.rm-modal h2{margin:0;padding:18px 20px 6px;font-size:19px;font-weight:700;color:#fff}
.rm-msub{padding:0 20px 12px;font-size:13px;color:#8b929a;line-height:1.5}
.rm-mitem{width:100%;display:flex;align-items:center;gap:12px;border:0;background:transparent;color:#dcdedf;font-size:15px;padding:0 20px;height:46px;cursor:pointer;text-align:left}
.rm-mitem:hover,.rm-mitem.gpfocus{background:#dcdedf;color:#0e141b}
.rm-mitem.rm-danger{color:#ff8a8a}
.rm-mitem.rm-danger:hover,.rm-mitem.rm-danger.gpfocus{background:#d4343c;color:#fff}
.rm-mv{margin-left:auto;font-size:13px;opacity:.7}
.rm-mgroup{border-top:1px solid #2a3039;padding:6px 0}
.rm-mlabel{font-size:11px;font-weight:700;letter-spacing:.09em;text-transform:uppercase;color:#8b929a;padding:8px 20px 4px}
.rm-mfoot{display:flex;gap:10px;justify-content:flex-end;padding:14px 20px;border-top:1px solid #2a3039}
.rm-tag{font-size:11px;font-weight:700;border-radius:3px;padding:2px 6px;background:#2f3540;color:#c3c8ce}
.rm-tag.rm-blue{background:#173a5a;color:#8cc8ff}
.rm-tag.rm-green{background:#1f3a1a;color:#9be08a}
.rm-cmpcol{flex:1;background:#141920;border:1px solid #2a3039;border-radius:6px;padding:12px}
.rm-grow{flex:1}
.rm-qhead h2{margin:0;font-size:19px;font-weight:700;color:#fff}
.rm-btn.rm-openlib{height:48px;background:#23303f}
.rm-btn.rm-openlib:hover,.rm-btn.rm-openlib.gpfocus{background:#dcdedf;color:#0e141b}
.rm-notes{white-space:pre-wrap;max-height:220px;overflow-y:auto;font-size:12px;line-height:1.4;color:#c3c8ce}
`;
