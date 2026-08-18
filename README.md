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

### Depurando travamentos de boot em ROMs reais

`core/examples/trace_boot.rs` é um tracer de execução ARM/Thumb usando [Capstone](https://www.capstone-engine.org/) (desmontador real e verificado — nada de decodificação feita na mão, que é exatamente o que gerou pistas falsas na investigação do FireRed). Roda a ROM de verdade (com ou sem uma BIOS real) e consegue:

- Apontar toda escrita cujo endereço efetivo (calculado com os valores reais dos registradores em tempo de execução, não uma suposição estática) bate com um endereço de I/O que você quer vigiar (`--watch`).
- Despejar um trace totalmente desmontado dos N instruções imediatamente antes de o PC alcançar um endereço conhecido de travamento (`--from-pc` + `--window`), com supressão de loops apertados pra não afogar o output.

```bash
cd core
cargo run --release --example trace_boot -- ../roms/algum-jogo.gba \
  --bios ../assets/bios/gba_bios.bin --frames 300 \
  --watch 0x4000004 --watch 0x4000200 --watch 0x4000208 \
  --from-pc 0x080008aa --window 4000
```

## BIOS

Este projeto **não distribui** (e não pode distribuir) a BIOS original da Nintendo. O núcleo tem uma BIOS "de mentira" feita na mão (HLE — High-Level Emulation, em `core/src/bios/`) reimplementando em Rust o efeito das chamadas de sistema que a BIOS real faria, mas ela sozinha só é suficiente para ROMs homebrew simples — a maioria dos jogos comerciais (Pokémon, Fire Emblem etc.) trava sem uma BIOS de verdade.

Em vez de pedir pro usuário arranjar um dump da BIOS da Nintendo, o app já vem com [Cult-of-GBA/BIOS](https://github.com/Cult-of-GBA/BIOS) embutido em `assets/bios/gba_bios.bin` — uma reimplementação **independente e open source** (licença MIT, ver `assets/bios/LICENSE`) da BIOS, escrita do zero em ARM assembly por terceiros, sem nenhum código da Nintendo. O app carrega ela automaticamente ao abrir; não é preciso selecionar nem carregar nada manualmente.

### Compatibilidade com jogos de Pokémon (FireRed/LeafGreen/Ruby/Sapphire/Emerald)

O Cult-of-GBA/BIOS não é 100% idêntico à BIOS real da Nintendo. Investigação a fundo (ver `core/examples/trace_boot.rs`) encontrou a causa: o despachante de IRQ dessa BIOS pula pro ponteiro de handler do jogo (`0x03FFFFFC`) sem checar se ele é nulo. Bem no início do boot, durante a própria animação do logo da BIOS, uma interrupção real de VBlank chega a disparar **antes** de o jogo instalar seu handler — e esse pulo por um ponteiro nulo acaba re-executando o vetor de reset, interferindo no processo de boot. O motor do Pokémon Gen 3 programa o `DISPSTAT` (registrador que habilita a interrupção de VBlank) através de uma tabela "sombra" em RAM que só é aplicada ao hardware de verdade dentro do próprio handler de VBlank — então, sem essa primeira interrupção chegando limpa no handler do jogo, `DISPSTAT` nunca é habilitado de verdade e o jogo trava esperando por uma interrupção que nunca vem.

Como correção definitiva exigiria remendar o binário da BIOS em Assembly ARM (ver `core/src/emulator/mod.rs`, comentário de `tick_peripherals`), o núcleo aplica uma correção de compatibilidade deliberada: a interrupção de VBlank/HBlank/VCounter é liberada com base apenas no `IE` (registrador de habilitação de interrupções), ignorando o bit de habilitação específico do `DISPSTAT` que o hardware real também exige. Isso é intencionalmente mais permissivo que o hardware real (compensando o bug da BIOS), mas resolve o travamento — FireRed testado e confirmado rodando (Fire Emblem e outros continuam funcionando normalmente).

## Save de jogos

O progresso salvo dentro do jogo (a memória de save do cartucho — SRAM ou Flash, dependendo do jogo) persiste automaticamente entre sessões, sem nenhuma ação manual:

1. Ao carregar um ROM, o app calcula um caminho de save baseado no nome do arquivo (`<nome-do-rom>.sav`, guardado no armazenamento privado do app). Se já existir um save nesse caminho, ele é restaurado antes do jogo começar a rodar.
2. Enquanto joga, o app verifica a cada ~1s se a memória de save mudou (o jogo salvou algo) e, se sim, grava em disco em uma thread separada — nunca trava o loop de áudio/vídeo.
3. Ao fechar o jogo (ou sair do app), um save final acontece de forma síncrona, garantindo que o progresso mais recente não se perca.

Esse trabalho é todo nativo (Kotlin lê/escreve os arquivos; ele só recebe o caminho já calculado pelo lado JS) — o núcleo Rust (`gba_core::cartridge`) já emulava SRAM/Flash desde o início, só faltava persistir em disco. Jogos sem chip de save (`SaveType::None`) simplesmente não geram nenhum arquivo. EEPROM (usado por alguns poucos jogos) ainda não é suportado pelo núcleo.

## Estrutura do projeto

```
core/                núcleo do emulador em Rust puro (CPU, PPU, APU, DMA, timers, cartucho)
                      — sem nenhuma dependência de React Native/Expo, com sua própria suíte de testes
core/examples/        ferramentas de depuração (trace_boot.rs — tracer de execução ARM/Thumb)
native/ffi/           ponte FFI entre core/ e as plataformas nativas: JNI (Android) e C ABI (iOS)
modules/gba-emulator/ módulo Expo que expõe o emulador ao React Native
                      (Kotlin no Android, Swift no iOS)
src/                  app React Native/Expo (tela principal, controles on-screen,
                      seleção de ROM)
assets/bios/          BIOS replacement open source (Cult-of-GBA/BIOS, MIT) embutida no app
roms/                 (ignorado pelo git) coloque aqui seus próprios dumps de ROM
```

## Aviso legal

Este projeto não inclui, distribui nem facilita o acesso a ROMs de jogos ou à BIOS da Nintendo. A BIOS embutida no app (`assets/bios/gba_bios.bin`) é o [Cult-of-GBA/BIOS](https://github.com/Cult-of-GBA/BIOS), um replacement independente e open source (MIT) — não é código da Nintendo. ROMs usadas com este emulador devem ser dumps legais de cartuchos que você mesmo possui.
