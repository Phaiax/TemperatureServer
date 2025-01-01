

## Connection

On Raspberry Pi 3:

 - Temperature Sensor Dallas
   - 3.3V -> RPI Pin 1 (3.3V)
   - Data -> RPI Pin 7 (GPIO4)
   - GND  -> RPI Pin 6 (GND)
 - 230V Heater Relais
   - 5V ( -> Relais -> Mosfet Drain) -> RPI Pin 4 (5V)
   - IO (Mosfet Gate) -> RPI Pin 11 (GPIO17)
   - GND (Mosfet Src) -> RPI Pin 9 (GND)

```
   ----------------------O                
kein Lötb┌──┐  1 |..|  2 | ┌──┐ n/a          
     n/a │  │  3 |..|  4 | │  │ or,5V     
     n/a │  │  5 |..|  6 | │  │ br,bl     
Lötbubble└──┘  7 |..|  8 | └──┘ n/a       
  or,GND ┌──┐  9 |..| 10 |        
  or,IO  │  │ 11 |..| 12 |                
     n/a │  │ 13 |..| 14 |                
     n/a └──┘ 15 |..| 16 |                
              17 |..| 18 |                
              19 |..| 20 |                
              21 |..| 22 |                
              23 |..| 24 |                
              25 |..| 26 |                
              27 |..| 28 |                
              29 |..| 30 |                
              31 |..| 32 |                
              33 |..| 34 |                
              35 |..| 36 |                
              37 |..| 38 |                
              39 |..| 40 |                
                         |                
                                          
      or=orange                           
      bl,br=blau,braun                    
      GND,IO,5V: ganz leicht erkennbar auf
                 Isolierbandfahnen        
      Lötbubble: Stecker mit Wiederstand  
```

## Installation

 - Optional: Install screen, .screenrc, atuin, tailscale
 - Install rustup, git, set git name/email
 - ssh-keygen, add deploy key, clone repo
 - Add `dtoverlay=w1-gpio,gpiopin=4` to `/boot/firmware/config.txt` (see https://www.kompf.de/weather/pionewiremini.html)
  - Restart
 - `cargo build --release`
 - Install service via ./install_server.sh
  - `sudo journald -u temperatures`
  - `sudo systemctl restart temperatures`
  - `LOG_FOLDER=~/log RUST_BACKTRACE=1 RUST_LOG=info WEBASSETS_FOLDER=server/webassets target/release/server`
 - ```
   mkdir ~/sensors
   cd ~/sensors 
   ln -s /sys/bus/w1/devices/28-00000b924ad3/w1_slave top
   ln -s /sys/bus/w1/devices/28-00000b92ca5c/w1_slave higher
   ln -s /sys/bus/w1/devices/28-00000b936566/w1_slave mid   
   ln -s /sys/bus/w1/devices/28-00000b94c80a/w1_slave lower
   ln -s /sys/bus/w1/devices/28-00000b960941/w1_slave bottom
   ln -s /sys/bus/w1/devices/28-0114502974aa/w1_slave outside
   ```
   (Order wrong except for outside)

## Testing

Note: Test file_db with one thread only:

    cargo test -- --test-threads 1
