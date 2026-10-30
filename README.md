# 後付け影MOD (Atozuke Kage MOD)

画像に AI で影を付け、その影だけを **別レイヤー** として取り出すツールです。元の画像の上に乗算で重ねると、影付きの画像になります。
影レイヤーなので、濃さ・合成モード・色味を後から自由に調整できます。

- 影を描くのは Qwen-Image 2.1（指示による画像編集）、人物の切り抜きは BiRefNet です。どちらもローカルの ComfyUI で動きます。
- AI の出力（約 1〜2.4MP）から影の比率を求め、元画像の輪郭に沿って原寸まで引き上げます。4K の画像でも、影の輪郭は元画像の解像度で付きます。
- アプリ本体は Rust 製の単体 exe です。ブラウザも Python も同梱していません。ComfyUI とモデルは各自で用意します。

## 必要なもの

| もの | 入手先 | 備考 |
| --- | --- | --- |
| NVIDIA GPU | | VRAM 12GB で動作確認しています。モデルは ComfyUI が RAM へ逃がしながら読み込みます |
| ComfyUI 0.40 以降 | [公式リリース](https://github.com/comfyanonymous/ComfyUI/releases)（Windows portable 推奨） | Qwen-Image 2.1 と BiRefNet のノードは標準で入っています。カスタムノードは不要です |
| モデル 4 ファイル（合計 17.7 GB） | 下の表 | **自動ダウンロードはしません** |

### モデル

各配布ページでライセンスを確認したうえでダウンロードし、次のどちらかで登録します。

- **設定 → モデル の「ファイルを選ぶ」**: ダウンロードしたファイルを選ぶと、照合してからモデルフォルダの「置き場所」に置きます。
  同じドライブならハードリンク（容量を使わず、元のファイルもそのまま）、別のドライブならコピーです。一致しないファイルは何も書き込みません。
- **自分で置く**: モデルフォルダ（設定 → モデル）の中の「置き場所」のフォルダに置いて「再チェック」を押します。

| 置き場所 | ファイル | サイズ | 配布ページ |
| --- | --- | --- | --- |
| `diffusion_models` | `qwen_image_2.1_int8_convrot.safetensors` | 7.26 GB | [Comfy-Org/Qwen-Image-2.1](https://huggingface.co/Comfy-Org/Qwen-Image-2.1/tree/main/diffusion_models) |
| `text_encoders` | `qwen3vl_8b_int8_convrot.safetensors` | 9.35 GB | [Comfy-Org/Qwen-Image-2.1](https://huggingface.co/Comfy-Org/Qwen-Image-2.1/tree/main/text_encoders) |
| `vae` | `qwen_image_2.1_vae_bf16.safetensors` | 0.68 GB | [Comfy-Org/Qwen-Image-2.1](https://huggingface.co/Comfy-Org/Qwen-Image-2.1/tree/main/vae) |
| `background_removal` | `birefnet.safetensors` | 0.44 GB | [Comfy-Org/BiRefNet](https://huggingface.co/Comfy-Org/BiRefNet/tree/main/background_removal) |

```
models/
├─ diffusion_models/qwen_image_2.1_int8_convrot.safetensors
├─ text_encoders/qwen3vl_8b_int8_convrot.safetensors
├─ vae/qwen_image_2.1_vae_bf16.safetensors
└─ background_removal/birefnet.safetensors
```

置いたファイルは、名前・サイズ・SHA-256 が公式ファイルと一致したときだけ使います（値は [src/models.rs](src/models.rs) にあります）。
一致しないファイルは読み込みません。SHA-256 の計算は初回だけで、以降はサイズと更新日時が変わらない限り再計算しません。

## 使い方

1. `AtozukeKageMOD.exe` を起動し、**設定 → ComfyUI** で ComfyUI のフォルダ（`ComfyUI_windows_portable` など）を選びます。
2. **設定 → モデル** でモデルフォルダを選びます。4 つとも「一致」になれば準備完了です（右上に「● モデル OK」と出ます）。
3. 画像を開き（ドロップでも可）、光の向きと影の濃さを選んで **影を生成** を押します。
4. キャンバス左上のメニューで、合成結果・乗算レイヤー・通常合成レイヤー・AI の出力を切り替えられます。合成モードと不透明度もここで変えられます。
5. **書き出し** で、影レイヤー・AI の出力・合成結果を PNG / JPEG / WebP / PSD（レイヤー付き）で保存します。

生成後に「影の濃さ」「影レイヤーをグレーにする」「二重線を消す」を変えると、AI を動かし直さずに影レイヤーだけを再計算します（1 秒前後）。
光の向き・追加の指示・背景の扱いを変えたときは、もう一度「影を生成」を押してください。

### ComfyUI の扱い

- アプリの起動時に、ComfyUI を専用の設定で裏で起動し、終了時に止めます。アプリが落ちた場合も一緒に止まります。
- 普段使いの ComfyUI とは別のポートで動きます。入出力のフォルダもアプリ側（`data/comfy`）を使い、カスタムノードは読み込みません。
- かかる時間の目安（RTX 4070 Ti 12GB、25 ステップ）:

  | 画像 | 1 回目 | 2 回目以降（モデルを GPU メモリに残す） |
  | --- | --- | --- |
  | 2.5MP 以下（AI は約 1MP で描く） | 約 1 分 | 約 30 秒 |
  | 2.5MP 超（4K など、AI は約 2.4MP で描く） | 約 100 秒 | 約 100 秒 |

  PC の起動直後など、モデルファイルがまだメモリに載っていないときは、1 回目にさらに数分かかります。
  影レイヤーの計算そのものは 4K でも 1 秒以下です。

### データの置き場所

`AtozukeKageMOD.exe` の隣の `data` フォルダに、設定（`settings.json`）・照合済みモデルの記録・ログ・ComfyUI の作業フォルダを置きます。
exe のフォルダに書き込めないときは `%LOCALAPPDATA%\AtozukeKageMOD` を使います。

## ライセンスと責任

- このアプリは MIT ライセンスです。
- ComfyUI とモデルは同梱していません。それぞれのライセンスに従って、利用者自身の責任で入手・利用してください。
  Qwen-Image 2.1 のライセンスには利用条件があります。商用利用の前に必ず確認してください。
- 生成した画像の扱いは利用者の責任です。

## ビルド

```
cargo build --release
```

`target/release/AtozukeKageMOD.exe` ができます。依存は Rust のクレートだけです（WebP エンコーダの libwebp はソースからビルドされます）。

### テスト

影レイヤーの計算は、ワークベンチのオーケストレーター（`orchestrator/src/orchestrator/shading.py`）の Rust 移植です。
`tests/regression.rs` で、Python 版が出力した期待値と画素単位で比べています（許容誤差は 8bit で ±1）。

```
cargo test --release
# 期待値を作り直す（Python 側のオーケストレーター環境で）
cd ../orchestrator && uv run --no-sync python ../kage-mod/tests/gen_fixtures.py ../kage-mod/tests/fixtures
```

`examples/smoke.rs` は、UI を使わずに ComfyUI の起動から影レイヤーの保存までを 1 回通すスモークテストです。

---

## English summary

Atozuke Kage MOD adds AI shading (Qwen-Image 2.1 via a local ComfyUI) to an image and extracts **only the shadows as a separate layer**.
Multiply it over the original to get the shaded image. The layer is solved at the model's resolution and upsampled along the original's edges,
so shadow edges stay sharp on 4K images. The app is a single Rust/egui exe.
You install ComfyUI (0.40+) and download the four model files above yourself. Nothing is downloaded automatically, and a file is used only if
its name, size and SHA-256 match the official release.
