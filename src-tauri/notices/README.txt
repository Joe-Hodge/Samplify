Samplify uses the HT-Demucs four-stem model by Meta AI, with the fp16-weight
ONNX export by StemSplit. The upstream model card identifies this export as MIT.
The build verifies the model against the SHA-256 in scripts/fetch-model.mjs.

Demucs project: https://github.com/facebookresearch/demucs
ONNX model card: https://huggingface.co/StemSplitio/htdemucs-onnx
Model license: Demucs-LICENSE.txt

The native runtime linked by ort 2.0.0-rc.12 is ONNX Runtime 1.24.2.
Its license and third-party notices are taken from Microsoft's 1.24.2 release.
Runtime project: https://github.com/microsoft/onnxruntime
Runtime notices: ONNX-Runtime-LICENSE.txt and ONNX-Runtime-ThirdPartyNotices.txt
