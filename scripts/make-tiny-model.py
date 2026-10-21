"""Writes crates/fuji-core/tests/fixtures/tiny-model: stand-ins for the SigLIP
vision/text ONNX exports (same input and output names, ~2 KB) so the
embedding plumbing can be tested without downloading the real model.

    python scripts/make-tiny-model.py   (needs `pip install onnx numpy`)
"""

import json
import pathlib

import numpy as np
import onnx
from onnx import TensorProto, helper, numpy_helper

OUT = pathlib.Path(__file__).resolve().parent.parent / "crates/fuji-core/tests/fixtures/tiny-model"
OUT.mkdir(parents=True, exist_ok=True)
rng = np.random.default_rng(7)
DIM = 8


def save(graph, name):
    model = helper.make_model(graph, opset_imports=[helper.make_opsetid("", 17)], producer_name="fujicloud-tests")
    model.ir_version = 8
    onnx.checker.check_model(model)
    onnx.save(model, OUT / name)


# Vision: mean colour per channel → linear projection. Also exposes a
# rank-2 `last_hidden_state` to check the pooled output is the one picked.
w = numpy_helper.from_array(rng.normal(size=(3, DIM)).astype(np.float32), "w")
b = numpy_helper.from_array(rng.normal(size=(DIM,)).astype(np.float32), "b")
save(
    helper.make_graph(
        [
            helper.make_node("ReduceMean", ["pixel_values"], ["last_hidden_state"], axes=[2, 3], keepdims=0),
            helper.make_node("MatMul", ["last_hidden_state", "w"], ["proj"]),
            helper.make_node("Add", ["proj", "b"], ["pooler_output"]),
        ],
        "vision",
        [helper.make_tensor_value_info("pixel_values", TensorProto.FLOAT, ["batch", 3, 224, 224])],
        [
            helper.make_tensor_value_info("last_hidden_state", TensorProto.FLOAT, ["batch", 3]),
            helper.make_tensor_value_info("pooler_output", TensorProto.FLOAT, ["batch", DIM]),
        ],
        [w, b],
    ),
    "vision.onnx",
)

# Text: token ids → linear projection.
wt = numpy_helper.from_array(rng.normal(size=(64, DIM)).astype(np.float32), "wt")
scale = numpy_helper.from_array(np.array(0.1, dtype=np.float32), "scale")
save(
    helper.make_graph(
        [
            helper.make_node("Cast", ["input_ids"], ["ids_f"], to=TensorProto.FLOAT),
            helper.make_node("Mul", ["ids_f", "scale"], ["ids_s"]),
            helper.make_node("MatMul", ["ids_s", "wt"], ["pooler_output"]),
        ],
        "text",
        [helper.make_tensor_value_info("input_ids", TensorProto.INT64, ["batch", 64])],
        [helper.make_tensor_value_info("pooler_output", TensorProto.FLOAT, ["batch", DIM])],
        [wt, scale],
    ),
    "text.onnx",
)

words = ["a", "photo", "of", "red", "blue", "green", "dog", "beach", "at", "sunset"]
vocab = {"<unk>": 0, "</s>": 1, **{w: i + 2 for i, w in enumerate(words)}}
eos = {"SpecialToken": {"id": "</s>", "type_id": 0}}
(OUT / "tokenizer.json").write_text(
    json.dumps(
        {
            "version": "1.0",
            "truncation": None,
            "padding": None,
            "added_tokens": [
                {"id": 1, "content": "</s>", "single_word": False, "lstrip": False, "rstrip": False, "normalized": False, "special": True}
            ],
            "normalizer": {"type": "Lowercase"},
            "pre_tokenizer": {"type": "Whitespace"},
            "post_processor": {
                "type": "TemplateProcessing",
                "single": [{"Sequence": {"id": "A", "type_id": 0}}, eos],
                "pair": [{"Sequence": {"id": "A", "type_id": 0}}, {"Sequence": {"id": "B", "type_id": 0}}, eos],
                "special_tokens": {"</s>": {"id": "</s>", "ids": [1], "tokens": ["</s>"]}},
            },
            "decoder": None,
            "model": {"type": "WordLevel", "vocab": vocab, "unk_token": "<unk>"},
        },
        indent=1,
    )
)
print("wrote", sorted(p.name for p in OUT.iterdir()))
