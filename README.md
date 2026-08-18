# GBA Emulator

Emulador de Game Boy Advance com núcleo próprio em Rust (CPU ARM7TDMI, PPU, APU, DMA, timers, cartucho) rodando dentro de um app Expo/React Native para Android (iOS ainda não testado/configurado).

## Requisitos

- **Node.js** (v20+) e **npm**
- **Android Studio**, com:
  - Android SDK
  - **NDK versão 27.1.12297006** (SDK Manager → SDK Tools → mostrar pacotes NDK antigos/específicos)
  - JDK 17 (normalmente já embutido no Android Studio)
- Um **celular Android físico** com Depuração USB ativada, ou um emulador Android já configurado
- **Rust** (via [rustup](https://rustup.rs)) — só necessário se for alterar o código em `core/` ou `native/ffi/`; para apenas rodar o app, a lib nativa já compilada vem versionada no repositório
- (Opcional, mas recomendado) uma **BIOS real de GBA** sua (dump de exatamente 16.384 bytes) — veja a seção [BIOS real](#bios-real-opcional-mas-recomendado)
- (Opcional) **ROMs de GBA** suas, para testar — a pasta `roms/` é ignorada pelo git

## Instalação

```bash
git clone <url-do-repositorio>
cd gba-emulator-for-android
npm install
```

### Configurando o NDK

O arquivo `.cargo/config.toml` aponta para o NDK num caminho fixo:

```
C:/Android/Sdk/ndk/27.1.12297006/...
```

Se o seu NDK estiver em outro caminho (ou versão), ajuste as 5 linhas desse arquivo (`ANDROID_NDK_HOME` + os `ar`/`linker` de `aarch64-linux-android` e `armv7-linux-androideabi`) para apontar para a instalação local. Isso só importa se você for recompilar o código Rust — para apenas rodar o app com o `.so` já versionado, pode ignorar esse passo.

## Rodando o app

A lib nativa compilada (`libgba_ffi.so`, o núcleo Rust cross-compilado para Android) **já vem versionada** em `modules/gba-emulator/android/src/main/jniLibs/arm64-v8a/`, então **não é necessário ter Rust instalado só para rodar o app**.

1. Conecte um celular Android via USB (com Depuração USB ativada) ou tenha um emulador Android já aberto.
2. Rode:
   ```bash
   npx expo run:android
   ```
   Isso builda o app nativo (via Gradle), instala no dispositivo conectado e sobe o Metro (servidor de desenvolvimento JS).
3. Se o app abrir mas ficar preso na tela de splash sem conseguir carregar o JS (comum em dispositivo físico via USB), rode:
   ```bash
   adb reverse tcp:8081 tcp:8081
   ```
   e reabra o app.

## Alterando o núcleo Rust (`core/` ou `native/ffi/`)

Sempre que mexer em `core/` ou `native/ffi/`, é preciso recompilar a lib nativa e copiar o `.so` atualizado para dentro do módulo Android antes de rodar `expo run:android` de novo:

```bash
rustup target add aarch64-linux-android   # só na primeira vez

cargo build -p gba-ffi --release --target aarch64-linux-android

# Windows (PowerShell):
copy target\aarch64-linux-android\release\libgba_ffi.so modules\gba-emulator\android\src\main\jniLibs\arm64-v8a\libgba_ffi.so
# Linux/macOS:
cp target/aarch64-linux-android/release/libgba_ffi.so modules/gba-emulator/android/src/main/jniLibs/arm64-v8a/libgba_ffi.so

npx expo run:android
```

> **Nota:** o `.cargo/config.toml` deste projeto aponta o linker/ar direto para os binários do NDK (em vez de usar `cargo-ndk`), porque `cargo-ndk` não compila no ambiente MinGW/Windows usado para desenvolver este projeto. Em Linux/macOS, `cargo-ndk` costuma ser uma alternativa mais simples, mas isso não foi testado neste repositório.

### Rodando os testes do core

```bash
cd core
cargo test --release
```

Os testes unitários rodam sem depender de nada externo. Alguns testes de integração (`real_rom_*.rs`) esperam ROMs de verdade em `roms/` (na raiz do projeto) e são pulados/falham sem elas — isso é esperado, veja o comentário em cada arquivo de teste.

## BIOS real (opcional, mas recomendado)

Este projeto **não distribui** (e não pode distribuir) a BIOS original da Nintendo. Por padrão, o núcleo roda com uma BIOS "de mentira" feita na mão (HLE — High-Level Emulation), reimplementando em Rust o efeito das chamadas de sistema que a BIOS real faria. Isso é suficiente para ROMs homebrew simples, mas a maioria dos jogos comerciais (Pokémon, Fire Emblem etc.) trava sem uma BIOS real.

Se você tiver o **seu próprio** dump legal de uma BIOS de GBA (arquivo de exatamente 16.384 bytes), pode carregá-la direto pelo app:

1. Abra o app → seção **BIOS** na tela inicial → **Load BIOS** → selecione o arquivo.
2. Ela fica salva no armazenamento do próprio app — não precisa selecionar de novo a cada vez que abrir o app.

## Estrutura do projeto

```
core/                núcleo do emulador em Rust puro (CPU, PPU, APU, DMA, timers, cartucho)
                      — sem nenhuma dependência de React Native/Expo, com sua própria suíte de testes
native/ffi/           ponte FFI entre core/ e as plataformas nativas: JNI (Android) e C ABI (iOS)
modules/gba-emulator/ módulo Expo que expõe o emulador ao React Native
                      (Kotlin no Android, Swift no iOS)
src/                  app React Native/Expo (tela principal, controles on-screen,
                      seleção de ROM/BIOS)
roms/                 (ignorado pelo git) coloque aqui seus próprios dumps de ROM
```

## Aviso legal

Este projeto não inclui, distribui nem facilita o acesso a ROMs de jogos ou à BIOS da Nintendo. ROMs e BIOS usados com este emulador devem ser dumps legais de cartuchos/hardware que você mesmo possui.
