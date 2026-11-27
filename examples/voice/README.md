# ボイスチャンネルで WAV を再生・録音する

この例では Akari が参加・退出と音声サーバーの割り当てを扱い、Songbird 0.6.1 が音声 WebSocket、DAVE、UDP、音声の送受信を扱います。音声の依存はこの例の Cargo プロジェクトにだけ入ります。[Songbird 0.6.1](https://github.com/serenity-rs/songbird/releases/tag/v0.6.1) · [Discord の音声接続仕様](https://docs.discord.com/developers/topics/voice-connections)

## 実行

Rust、CMake と、プラットフォームの C コンパイラーを使います。Windows では MSVC の開発用 PowerShell で実行できます。CMake がインストール済みの Visual Studio を識別できない場合は、そのシェルで `$env:CMAKE_GENERATOR = 'NMake Makefiles'` を指定します。

| 環境変数 | 値 |
| --- | --- |
| `DISCORD_TOKEN` | Bot のトークン。環境変数として渡します |
| `AKARI_VOICE_GUILD` | サーバー ID |
| `AKARI_VOICE_CHANNEL` | 通常のボイスチャンネル ID |
| `AKARI_VOICE_WAV` | 再生する PCM WAV ファイルのパス |
| `AKARI_VOICE_RECORD_WAV` | 録音を書き出す新しい WAV ファイルのパス |

再生と録音の環境変数はどちらか一方だけ設定します。Bot に接続の権限を与え、再生する場合は発言の権限も与えます。録音を始める前に、チャンネルの参加者へ知らせてください。リポジトリのルートから起動します。

```sh
cargo run --locked --manifest-path examples/voice/Cargo.toml
```

新しい割り当てが来ると音声サーバーに接続し直します。再生ではファイルの先頭に戻り、再生後はボイスチャンネルに留まります。

録音では、受け取った PCM を 48 kHz・16 bit・ステレオにミックスして、指定した新規ファイルに書き出します。既存ファイルは上書きしません。ファイルへの書き出しは別のタスクで行い、受信処理は書き出しやキューの空きを待ちません。キューは最大 64 フレームです。混雑で破棄したフレーム数は終了時に表示し、ファイルに書き出せなくなった場合は録音を停止してエラーを返します。

Ctrl+C で音声接続を止め、録音中はキューの書き出しと WAV ヘッダーの確定を待ってから終了します。

## Discord のコマンドで操作する

`AKARI_VOICE_RECORD_DIR` に録音の保存先を設定して起動すると、設定したサーバーだけに `/vc-join`、`/vc-record`、`/vc-stop`、`/vc-leave` を登録します。コマンドの実行には「サーバー管理」権限が必要です。

`/vc-join` と `/vc-record` は、チャンネル引数を省略すると実行者が参加している VC を使います。録音前に参加者へ知らせてください。`/vc-stop` は WAV を確定して実行先のチャンネルに投稿するため、Bot に送信とファイル添付の権限を与えます。`/vc-leave` は VC から退出し、録音中のファイルがあればローカルに保存します。

対話モードでは、サンプルのファイル上限 10 MiB に収まるフレームまでを書き出します。上限到達後に `/vc-stop` を使うと確定済みのファイルを送れます。送信できなかった場合もローカルの WAV は消しません。

## Discord に接続せず確認する

```sh
cargo test --locked --manifest-path examples/voice/Cargo.toml
```

合成音声で WAV の読み込み、PCM のミックス、WAV ヘッダーの確定、キューが満杯になったときの挙動を確認します。

Windows で Opus の同梱ビルドが失敗する場合は、MSVC 用にビルド済みの Opus を使えます。`OPUS_LIB_DIR` には `lib/opus.lib` を含むインストール先を指定します。
