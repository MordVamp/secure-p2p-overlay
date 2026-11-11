# pcap_no_plaintext

коммит `c732b144cdc0c8f545762b12837a8e36d499a21d` + незакоммиченные изменения (diff sha256 `cb3e3addf6d4`), N=15, k=3, α=3, R=3, relay=2, канал=ake, bootstrap=star, seed=1, конфиг `config/node_config.yaml`; драйвер `p2p-orchestrator scenario`

Захват `dumpcap` на lo, порты оверлея 7400–7414, во время 5 сообщений и 5 DHT STORE, содержащих маркер `PLAINTEXT-MARKER-2c6201c779fe43adaa429a0538f7c697`.

- маркер в трафике оверлея (overlay.pcapng, 428992 байт): **не найден**
- положительный контроль: тот же маркер в HTTP-запросах к панели узла (порты 7417–7431, открытый localhost-HTTP): найден — метод поиска работает
- протоколы в захвате оверлея — tshark_protocols.txt
