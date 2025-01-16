## POST
Simple rust program for transfering directory content over internet.  
can be started in:
* **client mode** - conect to a given host  
* **server mode** - listens on a given port for connections 

By default server is receiving directory content and client is sending directory content.

```sh
# server receiving: 
./target/debug/post -p 5555 -d ./test_in
# client   sending: 
./target/debug/post -p 5555 -d ./test_out -i localhost
```

If you want to use the the clent server in reverse mode :  
add -r parameter  

```sh
# server   sending: 
./target/debug/post -p 5555 -d ./test_out -r
# client receiving: 
./target/debug/post -p 5555 -d ./test_in  -r -i localhost
```


## help

**Usage:** post [OPTIONS]
```
Options:
  -d, --directory <DIRECTORY>  directory to send from or receive to [default: .]
  -i, --ip-host <IP_HOST>      ip/host_name to connect [default: ]
  -p, --port <PORT>            listening port [default: 5555]
  -r, --reverse                reverse mode
  -h, --help                   Print help
  -V, --version                Print version
```


## build

just use the normal rust building:

```
cargo build --release
```