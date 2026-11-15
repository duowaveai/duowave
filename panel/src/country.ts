// Страна дома (docs/plans/catalog-regions.md, 3; docs/plans/own-server.md): догадка — из компьютера человека, без
// сети. Часовой пояс говорит о месте (таблица ниже — из tzdb `zone.tab`, общественное достояние), язык браузера — о
// человеке, поэтому пояс первым. Никакого IP: страна не уходит со шлюза никуда.
// Названия стран — `Intl.DisplayNames` на языке пульта, флаги — эмодзи из кода (Windows покажет буквы — не страшно).

/**
 * Страна → её часовые пояса (tzdb 2026f, `zone.tab`, плюс старые имена): `RU:<хеш>,…;…`. Пояс — хеш его имени
 * (FNV-1a, 32 бита, base36), а не само имя: в коде нет названий городов, а сверка точная — коллизий в таблице нет
 * (проверено при сборке таблицы).
 */
const ZONES =
    'AD:dyquz1;AE:tqn3rh;AF:1xk0r5z;AG:ysqw7b;AI:ddf2ct;AL:1ntzdh7;AM:1avia0g;AO:10r9nrb;' +
    'AQ:2m2g6v,1ds5c81,wpd2ld,16mkq5e,1ocjhxf,zp3rpl,l8uxs5,1so3vqz,1k1v5td,1gsrfyc;' +
    'AR:1k5gwa1,161x4l2,19edzw1,qwnzdt,1i9115n,xownlr,6cg1o5,hg8497,v5gh8u,clsp16,fwa4rb,1jt77hu,19l51f7;AS:7xvfk8;' +
    'AT:my4t9v;' +
    'AU:pqmiwb,1pch9jm,rbuc8o,1n1t9ip,zgwjqq,w50emf,3006v2,19ao9la,408oon,1derb1,c8hz4l,137mlpm,ycgpwe,hexwfq,3rtevk;' +
    'AW:1ykt09b;AX:p0ufka;AZ:1p1ii4l;BA:nj50pb;BB:7tba1q;BD:1gqsobr,flv42k;BE:1c1pmmp;BF:aw6dm2;BG:18pb0c2;' +
    'BH:au4zkd;BI:13gbsz;BJ:1dpzokn;BL:2uafpd;BM:g5bhly;BN:1g60ep9;BO:1l0e4br;BQ:1tf6hoj;' +
    'BR:1pqnj5t,zt8trh,1rqthca,1f1ota0,122jfy5,1qxnmj2,12j62et,10qb8nd,tq2h9y,1avjq7r,wnqr67,5gim7h,1mxaz4w,1f2d6z,1uw2fe5,19a1ar2,lk3rth;' +
    'BS:1199c6h;BT:i2yco9,at1503;BW:edfel;BY:14rzyei;BZ:1bn6hwr;' +
    'CA:tgk1gu,1nshlej,eldca1,ar8h2c,1g95cdc,49mpii,1h86kjt,129ct17,1ia8jbc,1q044ud,2fhzkf,1enanxa,sroyrk,11wyiyl,1tghz9u,n6bcl5,13inhnw,yl47kb,1c6005m,a64thj,ahx20n,tzilhe,11cnuyo,1rvyi5g,1olgrj1,zqv22v,1xucgdz,1g81n5s,1ax774n;' +
    'CC:1anciq4;CD:zns2uo,4bhlzm;CF:1oobnrg;CG:17cg5wm;CH:17nfkuj;CI:1h08cj7;CK:1nttkc0;' +
    'CL:1bf8gp6,hw0192,bg7hep,18nt0qj,sww94o;CM:bfh0z4;CN:1ea6utd,1bl334z,10rreje,1pk52ds,1m1iw4s;CO:1v5ureu;' +
    'CR:gdgw04;CU:1woxu0b,1v7h66k;CV:fa78sw;CW:n8lm56;CX:awybtp;CY:16xpora,w152i3;CZ:1or4rq4;DE:dnyifo,1qcs4ft;' +
    'DJ:e946qu;DK:w5ui7s;DM:m3y1oe;DO:14d48th;DZ:165t4nr;EC:1z7u62,1le4blo;EE:133sji6;EG:1t2v2ws,1nwdpmw;' +
    'EH:16zm6j0;ER:8hrka3;ES:y8m5qt,kw3w4,nbbwxu;ET:1lw1xdz;FI:by8bsl;FJ:1ph8eyd;FK:8wyjus;' +
    'FM:a2hx7p,fac47k,1tsj01w;FO:1xf1bqb;FR:uijlvz;GA:1o6sj7m;GB:1guuzwu,pr44q4;GD:dl3y7i;GE:6qsdbq;GF:5dtvud;' +
    'GG:jzcvpe;GH:12f7qdk;GI:10ktgsc;GL:17wo0a5,tmzrh8,hte014,542chg,1lhlv39;GM:arpj14;GN:1mvppkh;GP:sywo27;' +
    'GQ:7bjz7e;GR:1rc1nfd;GS:fn6vk;GT:1r1yxvp;GU:3ogtbz;GW:cn69ah;GY:q299nb;HK:16zq0ly,y4n2nq;HN:3mum42;HR:1ccgi1n;' +
    'HT:3rff0e;HU:14s2rhi;ID:1jzwhr6,1pkc79x,1p46k4n,1d3mejh;IE:rvosgm,1vgct8a;IL:1fbwqji,okjdya;IM:1w89q9e;' +
    'IN:1u8sj9h,15peawr;IO:1ksebcu;IQ:m9iawn;IR:9z3ma,1kjaqb9;IS:rl5fcs,1it1d39;IT:1kakmal;JE:7a832o;' +
    'JM:y4frn2,1g7pns9;JO:iqwk8u;JP:13nkzge,193rgu3;KE:bdxg7a;KG:bf1pb;KH:158ghoq;KI:1w9ns1f,1tw7co6,1jdborq;' +
    'KM:wjjaem;KN:4j70rx;KP:ggkspu;KR:k4s3y0,vvrpjz;KW:1agdez9;KY:5uy2yx;' +
    'KZ:1j0orcc,12zuna7,9nsw1a,5ya95e,xftnvc,13hsxsc,eibve8;LA:btmoo9;LB:1plk0f3;LC:1a09fuq;LI:1c5gu1o;LK:lfbzo1;' +
    'LR:18vazzn;LS:1vsl6hj;LT:10ap6vq;LU:1ojiisk;LV:thrw2d;LY:1b84j7,1nqinm2;MA:17mzro5;MC:h0rq4b;MD:bz2rqm;' +
    'ME:z9ynjk;MF:hdl877;MG:1ans3fp;MH:aybuvx,1lgljj;MK:1y9itgk;ML:198n16d;MM:99c2oo,ejjkmy;' +
    'MN:13yrpko,1ly0j0v,40krfz;MO:uvdcm9;MP:b8kes7;MQ:9wkd17;MR:1jwxon6;MS:ewz527;MT:jzv5t5;MU:130h4le;MV:1o8s7re;' +
    'MW:1ge0q4f;' +
    'MX:1a2fybb,1dtftqw,1j9pk04,f4nrkv,gt67yv,1nf5koy,146ke94,1rhye1l,133kuok,rgh84i,3qvvww,12j0an8,86v3dt;' +
    'MY:ptyyys,manykh;MZ:18gb540;NA:1g5ax0v;NC:1yirj0a;NE:shvjkd;NF:7v0siu;NG:5uywiy;NI:ypml46;NL:1mkujeg;' +
    'NO:y1sglr;NP:sl3ykb,teicd7;NR:vmycd2;NU:f85y5y;NZ:96ssq0,10pv0nj,d0gzwx;OM:1tf0x61;PA:5k6hi8;PE:19e445l;' +
    'PF:1iy5may,1d4dl39,x2pb46;PG:17s82o2,1dy3w4m;PH:c1fqw8;PK:1nbgmz1;PL:156usib,hbescl;PM:jytuzw;PN:s6pfjt;' +
    'PR:1xubob9;PS:1ois09d,4m3kfe;PT:1rud1p7,m7mo0n,1ruc108,ap23k7;PW:1o303l0;PY:1nuqjua;QA:r5glz1;RE:416ea1;' +
    'RO:1o8uxpf;RS:18nnu42;' +
    'RU:wsepwo,52q1o,9vl1w7,1k5vazf,or9pb1,a7o8ja,128f0so,5hiq37,soogns,1mcfw32,owci1b,93r2s1,12tbxzu,i8bj7j,vyydio,1qc4pjz,6y6cqt,1gypyyk,vi6zsz,1e7u63k,z4v90j,7tk0mt,e8bf1l,1qrv6n1,1bsmegd,k1f9ox,8ammu1;' +
    'RW:1gpcivf;SA:1gzpf7f;SB:6p2s5o;SC:mn55lu;SD:1e9mq5j;SE:sa9v2k;SG:1ol2vua,1qkccht;SH:nf9f4j;SI:1b5kzp7;' +
    'SJ:1b3zxcr;SK:1in33hz;SL:jvikxq;SM:dm770j;SN:1tz5o9r;SO:11fpl7r;SR:1njo6nm;SS:ly1ba8;ST:e62qut;SV:pgczgo;' +
    'SX:j5kk1w;SY:1jnxslf;SZ:sg8h7q;TC:f06ukb;TD:1pm53z4;TF:yti9bp;TG:1iy0uuj;TH:g2bmil;TJ:74lf6u;TK:ft3tea;' +
    'TL:7o00uw;TM:h590vz;TN:19dhj3f;TO:5ohms6;TR:2cppoy,1jdkw2;TT:1haseef;TV:1fyc3j7;TW:17ag32o,y3oj3r;TZ:6a7bp4;' +
    'UA:19nq8k4,w1gvbr,19yh417,f2mw68,15om48n;UG:p9amyj;UM:i8rhec,dlz2gt;' +
    'US:23s7ks,yz03dj,b3blsj,1y7q4r9,q6hrki,1bkld80,9tui0x,125yjzo,2wdjna,1llowr2,f2n7is,1sbecyi,1jmikf9,bta9ct,1ql6sde,ys4sk2,xkf352,sz0lss,1kulutu,1dowofz,16ih80k,10hutwk,16pfpy,3q7b6w,16rf3jg,19kzwhz,ndfnqt,1lhuki1,13vd81r,gtg9qa,jpoxm3,1tb4cux,1noilzp,1loybqb,1m10afj;' +
    'UY:j9vuls;UZ:rpxgi2,1aup342;VA:1jbxunc;VC:5wrb37;VE:1kexize;VG:1siamrv;VI:18b2yvk;VN:1kmntuh,1ayn2h9;' +
    'VU:u5jmly;WF:ohcxp1;WS:15mtzcy;YE:pm1tjm;YT:1m96gug;ZA:e19ope;ZM:18xxvsj;ZW:c7s3ln';

let byZone: Map<string, string> | null = null;
let codes: string[] | null = null;

function table(): Map<string, string> {
    if (byZone) return byZone;
    byZone = new Map();
    codes = [];
    for (const part of ZONES.split(';')) {
        const [code, zones] = part.split(':');
        codes.push(code);
        for (const zone of zones.split(',')) byZone.set(zone, code);
    }
    return byZone;
}

/** Все страны, у которых есть часовой пояс, — коды ISO 3166-1. */
export function countryCodes(): string[] {
    table();
    return codes ?? [];
}

/** Страна часового пояса: `Europe/Berlin` → `DE`; не знаем — null. */
export function zoneCountry(zone: string | undefined | null): string | null {
    return zone ? (table().get(zoneKey(zone)) ?? null) : null;
}

/** Ключ пояса в таблице: FNV-1a 32 бита от UTF-8 имени, base36. */
export function zoneKey(zone: string): string {
    let hash = 0x811c9dc5;
    for (const byte of new TextEncoder().encode(zone)) {
        hash ^= byte;
        hash = Math.imul(hash, 0x01000193) >>> 0;
    }
    return hash.toString(36);
}

/** Страны из языков браузера: `ru-RU`, `en-GB` → `RU`, `GB`; язык без страны (`ru`) ничего не даёт. */
export function languageCountries(languages: readonly string[]): string[] {
    const known = countryCodes();
    const found: string[] = [];
    for (const tag of languages) {
        const region = tag.split(/[-_]/).slice(1).find((part) => /^[A-Za-z]{2}$/.test(part));
        const code = region?.toUpperCase();
        if (code && known.includes(code) && !found.includes(code)) found.push(code);
    }
    return found;
}

/**
 * Догадка, лучшая первой: страна часового пояса, затем страны языков браузера. Пусто — гадать не из чего (пояс UTC,
 * язык без страны).
 */
export function guessCountries(): string[] {
    let zone: string | undefined;
    try {
        zone = Intl.DateTimeFormat().resolvedOptions().timeZone;
    } catch {
        zone = undefined;
    }
    const out: string[] = [];
    const byTime = zoneCountry(zone);
    if (byTime) out.push(byTime);
    const languages = typeof navigator !== 'undefined' ? (navigator.languages ?? [navigator.language]) : [];
    for (const code of languageCountries(languages)) if (!out.includes(code)) out.push(code);
    return out;
}

/** Флаг-эмодзи страны: `NL` → 🇳🇱; не код страны — пусто. */
export function flagOf(code: string | null | undefined): string {
    if (!code || !/^[A-Za-z]{2}$/.test(code)) return '';
    const base = 0x1f1e6;
    return String.fromCodePoint(...code.toUpperCase().split('').map((letter) => base + letter.charCodeAt(0) - 65));
}

const names = new Map<string, Intl.DisplayNames | null>();

/** Название страны на языке пульта; браузер не знает — сам код. */
export function countryName(code: string, locale: string): string {
    let display = names.get(locale);
    if (display === undefined) {
        try {
            display = new Intl.DisplayNames([locale], { type: 'region' });
        } catch {
            display = null;
        }
        names.set(locale, display);
    }
    try {
        return display?.of(code.toUpperCase()) ?? code;
    } catch {
        return code;
    }
}

/** Все страны с названиями на языке пульта, по алфавиту. */
export function countryList(locale: string): { code: string; name: string }[] {
    return countryCodes()
        .map((code) => ({ code, name: countryName(code, locale) }))
        .sort((a, b) => a.name.localeCompare(b.name, locale));
}
