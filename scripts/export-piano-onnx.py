import torch, numpy as np, sys
sys.path.insert(0, '.')
import models_inf as M
ck = torch.load('ckpt.pth', map_location='cpu', weights_only=False)
print(ck.keys() if isinstance(ck, dict) else type(ck))
sd = ck['model']['note_model']
m = M.Regress_onset_offset_frame_velocity_CRNN(100, 88)
print(m.load_state_dict(sd, strict=True))
m.eval()
class W(torch.nn.Module):
    def __init__(s, m): super().__init__(); s.m = m
    def forward(s, x):
        o = s.m(x)
        return o['reg_onset_output'], o['reg_offset_output'], o['frame_output'], o['velocity_output']
w = W(m).eval()
x = torch.randn(2, 160000) * 0.1
with torch.no_grad():
    ref = w(x)
    print([r.shape for r in ref])
torch.onnx.export(w, x, 'piano.onnx', input_names=['audio'], output_names=['onset','offset','frame','velocity'],
    dynamic_axes={'audio': {0: 'batch'}, 'onset': {0: 'batch'}, 'offset': {0: 'batch'}, 'frame': {0: 'batch'}, 'velocity': {0: 'batch'}}, opset_version=17, dynamo=False)
import onnxruntime as ort
s = ort.InferenceSession('piano.onnx', providers=['CPUExecutionProvider'])
out = s.run(None, {'audio': x.numpy()})
for a, b in zip(out, ref): print(a.shape, float(np.abs(a - b.numpy()).max()))
