# 後付け影MOD

画像に AI で影を付け、影だけを別レイヤーとして取り出す Windows アプリです。

- 影レイヤーは元画像に乗算して使います。濃さ、合成モード、不透明度は生成後に変更できます
- 影の輪郭は元画像の解像度に合わせます
- 書き出し形式: PNG / JPEG / WebP / レイヤー付き PSD（PSD は Photoshop 実機で未確認）

## 必要なもの

- NVIDIA GPU（VRAM 12GB で動作確認）
- [ComfyUI](https://github.com/comfyanonymous/ComfyUI/releases) 0.40 以降
- 次のモデル 4 ファイル（合計 17.7 GB）

モデルは自動でダウンロードしません。名前、サイズ、SHA-256 が公式と一致するファイルだけを読み込みます。

| 置き場所 | ファイル | サイズ | 配布元 |
| --- | --- | --- | --- |
| `diffusion_models` | `qwen_image_2.1_int8_convrot.safetensors` | 7.26 GB | [Comfy-Org/Qwen-Image-2.1](https://huggingface.co/Comfy-Org/Qwen-Image-2.1/tree/main/diffusion_models) |
| `text_encoders` | `qwen3vl_8b_int8_convrot.safetensors` | 9.35 GB | [Comfy-Org/Qwen-Image-2.1](https://huggingface.co/Comfy-Org/Qwen-Image-2.1/tree/main/text_encoders) |
| `vae` | `qwen_image_2.1_vae_bf16.safetensors` | 0.68 GB | [Comfy-Org/Qwen-Image-2.1](https://huggingface.co/Comfy-Org/Qwen-Image-2.1/tree/main/vae) |
| `background_removal` | `birefnet.safetensors` | 0.44 GB | [Comfy-Org/BiRefNet](https://huggingface.co/Comfy-Org/BiRefNet/tree/main/background_removal) |

## 使い方

1. `AtozukeKageMOD.exe` を起動し、画面の手順で ComfyUI とモデルを設定
2. 画像を開く
3. 光源の向きを選び、「影を生成」を押す
4. 濃さとレイヤーを調整
5. 書き出す

処理時間は RTX 4070 Ti 12GB で 1 枚 30 秒〜1 分、4K で約 100 秒です。起動後の初回は、モデルの読み込みで数分長くかかります。

| キー | 動作 |
| --- | --- |
| Ctrl+O | 開く |
| Ctrl+S | 書き出し |
| Ctrl+0 / Ctrl+1 | 全体 / 等倍 |
| Ctrl++ / Ctrl+- | 拡大 / 縮小 |
| C（押している間） | 元画像を表示 |

## 仕様

- 濃さ、背景、モノクロ、ぼかしの変更は AI を再実行せずに反映します。光源の向きと追加の指示を変えたら再生成が必要です
- 透過画像の透明部分は保持します。半透明の縁は、PSD を暗い背景に重ねると見え方が少し変わります。JPEG は白地に合成します
- ComfyUI は専用のポートと設定で起動し、アプリと一緒に終了します。カスタムノードは読み込みません
- 設定、ログ、作業ファイルは exe と同じ場所の `data` に保存します。書き込めない場合は `%LOCALAPPDATA%\AtozukeKageMOD` を使います

## ライセンス

本体は [MIT](LICENSE) です。ComfyUI とモデルは同梱していません。各ライセンスに従って利用してください。
Qwen-Image 2.1 には利用条件があります。商用利用の前に確認してください。

## ビルド

```
cargo build --release
cargo test --release --lib --tests
```

---

Windows app that adds AI shading to an image and extracts it as a separate shadow layer. Requires ComfyUI 0.40+ and the four model files above. Models are never downloaded automatically.
