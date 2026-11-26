# Discord で Bot を用意する

コードを書く前に、Discord Developer Portal で Bot のアカウントを作ります。

## 1. アプリケーションを作る

1. [Discord Developer Portal](https://discord.com/developers/applications) を開き、**New Application** を押します。
2. 好きな名前を付けて作成します。

## 2. Bot トークンを取得する

1. 左のメニューから **Bot** を開きます。
2. **Reset Token** を押してトークンを表示し、コピーします。

> [!WARNING]
> トークンは Bot のパスワードです。他人に見せたり、GitHub に公開したりしないでください。漏れた場合は、すぐに **Reset Token** で作り直します。コードにも直接書かず、環境変数で渡します。

## 3. Message Content Intent を有効にする

**Bot** タブの **Privileged Gateway Intents** で、**Message Content Intent** を有効にして保存します。

これは、`!ping` のようなテキストコマンドでメッセージの本文を読むために必要です。スラッシュコマンドやボタンだけを使う Bot なら不要です。

> [!TIP]
> 有効にし忘れると、接続時に Gateway がコード `4014` で閉じます。[よくある質問](../reference/faq.md) を参照してください。

## 4. Bot をサーバーに招待する

1. 左のメニューから **OAuth2** を開き、**URL Generator** を選びます。
2. **Scopes** で `bot` と `applications.commands` を選びます。
3. **Bot Permissions** で次を選びます。

   | 権限 | 用途 |
   | --- | --- |
   | View Channel | チャンネルを見る |
   | Send Messages | 返信する |
   | Read Message History | 返信先の元メッセージを読む（返信に必要） |
   | Embed Links | Embed を表示する（Embed を使う場合） |

4. 下に表示された URL をブラウザーで開き、テスト用のサーバーに Bot を追加します。

## 5. サーバー ID を控える（スラッシュコマンドを使う場合）

スラッシュコマンドは、サーバーを指定して登録します。

1. Discord の **ユーザー設定 → 詳細設定** で **開発者モード** を有効にします。
2. テスト用サーバーのアイコンを右クリックし、**サーバー ID をコピー** を選びます。

## トークンを渡す

Akari は環境変数 `DISCORD_TOKEN` からトークンを読みます。ターミナルで設定します。

**Bash（macOS / Linux）**

```bash
read -rs -p 'Discord Bot token: ' DISCORD_TOKEN; printf '\n'
export DISCORD_TOKEN
```

**PowerShell（7.2 以上）**

```powershell
$env:DISCORD_TOKEN = Read-Host 'Discord Bot token' -MaskInput
```

これで準備ができました。[はじめての Bot](first-bot.md) に進みます。
