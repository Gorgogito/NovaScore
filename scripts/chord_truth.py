import xml.etree.ElementTree as ET, json, sys, collections
X='D:/ProjectsIA/NovaScore/artefactos/partituras/musetrainer-xml'
OUT='D:/ProjectsIA/NovaScore/artefactos/acordes'
STEP={'C':0,'D':2,'E':4,'F':5,'G':7,'A':9,'B':11}
kinds=collections.Counter()
for name in ['Bella_Ciao','Happy_Birthday_To_You_C_Major','Hungarian_Dance_No_5_in_G_Minor','The_Entertainer_-_Scott_Joplin']:
    root=ET.parse(X+'/'+name+'.musicxml').getroot()
    part=root.find('part')  # primer pentagrama: las armonias suelen estar en el primero
    div=1; res=[]
    for mi,m in enumerate(part.findall('measure')):
        pos=0; mx=0
        for el in m:
            if el.tag=='attributes':
                d=el.find('divisions')
                if d is not None: div=int(d.text)
            elif el.tag=='note':
                if el.find('chord') is None:
                    dur=el.find('duration'); pos+= int(dur.text) if dur is not None else 0
            elif el.tag=='backup': pos-=int(el.find('duration').text)
            elif el.tag=='forward': pos+=int(el.find('duration').text)
            elif el.tag=='harmony':
                r=el.find('root'); rs=r.find('root-step').text; ra=r.find('root-alter'); a=int(float(ra.text)) if ra is not None else 0
                k=el.find('kind'); kind=k.text; kinds[kind]+=1
                b=el.find('bass'); bp=None
                if b is not None:
                    bs=b.find('bass-step').text; ba=b.find('bass-alter'); bp=(STEP[bs]+(int(float(ba.text)) if ba is not None else 0))%12
                off=el.find('offset'); o=int(off.text) if off is not None else 0
                res.append({'measure':mi,'tick':round((pos+o)*480/div),'root':(STEP[rs]+a)%12,'kind':kind,'bass':bp,'text':k.get('text','')})
    json.dump(res,open(OUT+'/'+name+'.json','w'))
    print(name,len(res),res[:3])
print(kinds)
