import json, time, sys
src, out = sys.argv[1], sys.argv[2]
g = json.load(open(src))
b = g['bft']; keep = [0, 4, 5, 6, 7]
bonds = b['staking']['bonds']
assert len(b['validators']) == 8 and len(bonds) == 8
drop = [bonds[i] for i in (1, 2, 3)]
alloc = {a['address']: a for a in g['allocations']}
before = sum(a['balance'] for a in g['allocations']) + sum(x['bond'] for x in bonds)
for d in drop:  # a dropped bond returns to its operator so total supply is unchanged
    alloc.setdefault(d['operator'], {'address': d['operator'], 'balance': 0})['balance'] += d['bond']
g['allocations'] = list(alloc.values())
b['validators'] = [b['validators'][i] for i in keep]
b['staking']['bonds'] = [bonds[i] for i in keep]
after = sum(a['balance'] for a in g['allocations']) + sum(x['bond'] for x in b['staking']['bonds'])
assert before == after, (before, after)
g['timestamp'] = int(time.time())
json.dump(g, open(out, 'w'), indent=2)
vb = [x['bond'] for x in b['staking']['bonds']]
print('supply', after, 'bonds', vb, 'oracle share', vb[0] / sum(vb))
