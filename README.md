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

- **設定 → モデル の「ファイルを選ぶ」**: ダウンロードしたファイルを選ぶと、照合してからモデルディレクトリの「置き場所」に置きます。
  「ダウンロードしたファイルをまとめて選ぶ」なら、4 ファイルを一度に選べます。
  同じドライブならハードリンク（容量を使わず、元のファイルもそのまま）、別のドライブならコピーです。一致しないファイルは何も書き込みません。
- **自分で置く**: モデルディレクトリ（設定 → モデル）の中の「置き場所」のディレクトリに置いて「再チェック」を押します。

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

作業は画面の左から右へ進みます。

1. `AtozukeKageMOD.exe` を起動すると、中央に準備の手順が表示されます。ComfyUI のディレクトリ（`ComfyUI_windows_portable` など）を選び、モデルを揃えます。
   手順は **設定 → ComfyUI** と **設定 → モデル** でも変更できます。4 つとも「一致」になれば、右上に「● Ready」と表示されます。
2. 画像を開き（ウィンドウへのドロップでも可）、左のパネルで光源の向きや影の濃さを選んで **影を生成** を押します。
3. 中央で結果を確認します。キャンバスの下の「比較」で元画像と左右に見比べられます。C キーを押している間は元画像を表示します。
4. 右の **レイヤー** パネルに「影」「AI の出力」「元画像」が並びます。画像編集ソフトと同じく、目のアイコンで表示／非表示（Alt＋クリックでそのレイヤーだけ表示、もう一度で元に戻す）、
   行を選んで上の欄で合成モード（乗算・焼き込み・通常（黒＋透明度）など）と不透明度を変えます。
5. 右下の **書き出し** で、影レイヤー・AI の出力・合成結果を PNG / JPEG / WebP / PSD（レイヤー付き）で保存します。合成結果と PSD は、レイヤーパネルの表示・合成モード・不透明度のままです。

生成後に「影の濃さ」「背景にも影を付けて生成する」「影色をモノクロにする」「（実験的）影をぼかしてなじませる」を変えると、AI を再実行せずに影レイヤーだけを再計算します（1 秒前後）。背景をオンで生成したあとオフにするときだけ、人物の検出（BiRefNet）を 1 回実行します。
光源の向きや追加の指示を変えると、「影を生成」の上に案内が表示されます。もう一度「影を生成」を押してください。

透過画像の透明部分は、キャンバスでも書き出しでも透明のままです。ただし、半透明の縁を暗い背景に重ねると、PSD の見え方は合成結果と少し異なります。
JPEG は透明を持てないので、白地に合成します。WebP は画質 100 で可逆圧縮になります。
画像に色プロファイルがあれば、合成結果・AI の出力・PSD に引き継ぎます。

| キー | 動作 |
| --- | --- |
| Ctrl+O | 画像を開く |
| Ctrl+S | 書き出し |
| Ctrl+0 / Ctrl+1 | 全体表示 / 等倍 |
| Ctrl++ / Ctrl+- | 拡大 / 縮小 |
| C（押している間） | 元画像を表示 |

キャンバスの周囲の色は、キャンバスの右クリックで変えられます。

### ComfyUI の扱い

- アプリの起動時に、ComfyUI を専用の設定で裏で起動し、終了時に止めます。アプリが落ちた場合も一緒に止まります。
- 普段使いの ComfyUI とは別のポートで動きます。入出力のディレクトリもアプリ側（`data/comfy`）を使い、カスタムノードは読み込みません。
- かかる時間の目安（RTX 4070 Ti 12GB、25 ステップ）:

  | 画像 | 1 回目 | 2 回目以降（モデルを GPU メモリに残す） |
  | --- | --- | --- |
  | 2.5MP 以下（AI は約 1MP で描く） | 約 1 分 | 約 30 秒 |
  | 2.5MP 超（4K など、AI は約 2.4MP で描く） | 約 100 秒 | 約 100 秒 |

  PC の起動直後など、モデルファイルがまだメモリに載っていないときは、1 回目にさらに数分かかります。
  影レイヤーの計算そのものは 4K でも 1 秒以下です。

### データの置き場所

`AtozukeKageMOD.exe` の隣の `data` ディレクトリに、設定（`settings.json`）・照合済みモデルの記録・ログ・ComfyUI の作業ディレクトリを置きます。
exe のディレクトリに書き込めないときは `%LOCALAPPDATA%\AtozukeKageMOD` を使います。

## ライセンスと責任

- このアプリは [MIT ライセンス](LICENSE) です。
- ComfyUI とモデルは同梱していません。それぞれのライセンスに従って、利用者自身の責任で入手・利用してください。
  Qwen-Image 2.1 のライセンスには利用条件があります。商用利用の前に必ず確認してください。
- 生成した画像の扱いは利用者の責任です。

## ビルド

```
cargo build --release
```

`target/release/AtozukeKageMOD.exe` ができます。依存は Rust のクレートだけです（WebP エンコーダの libwebp はソースからビルドされます）。

### テスト

影レイヤーの計算は、Python の参照実装（[tests/reference/shading.py](tests/reference/shading.py)）の Rust 移植です。
`tests/regression.rs` で、Python 版が出力した期待値と画素単位で比べています（許容誤差は 8bit で ±1）。

```
cargo test --release
# 期待値を作り直す（numpy と Pillow が必要）
uv run --with numpy --with pillow python tests/gen_fixtures.py tests/fixtures
```

`examples/smoke.rs` は、UI を使わずに ComfyUI の起動から影レイヤーの保存までを 1 回通すスモークテストです。

---

## English summary

Atozuke Kage MOD adds AI shading (Qwen-Image 2.1 via a local ComfyUI) to an image and extracts **only the shadows as a separate layer**.
Multiply it over the original to get the shaded image. The layer is solved at the model's resolution and upsampled along the original's edges,
so shadow edges stay sharp on 4K images. The app is a single Rust/egui exe.
You install ComfyUI (0.40+) and download the four model files above yourself. Nothing is downloaded automatically, and a file is used only if
its name, size and SHA-256 match the official release.
