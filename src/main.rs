use std::{
    fs::{self, File},
    io::{BufReader, Read, Write},
    net::SocketAddr,
    path::{Path, PathBuf},
};

use anyhow::Error;
use clap::Parser;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use walkdir::WalkDir;

/// Post directory to another host.
/// i.e. send and receive directory content via socket.
/// server mode (receiving): -d [dir] -p port
/// client mode (  sending): -d [dir] -p port -i ip_host
/// server mode (  sending): -d [dir] -p port -r
/// client mode (receiving): -d [dir] -p port -r -i ip_host

#[derive(Parser, Debug)]
#[clap(verbatim_doc_comment)]
#[command(version, about)]
struct Args {
    /// directory to send from or receive to
    #[arg(short, long, default_value = ".")]
    directory: String,

    /// ip/host_name to connect
    #[arg(short, long, default_value = "")]
    ip_host: String,

    /// listening port
    #[arg(short, long, default_value_t = 5555)]
    port: u16,

    /// reverse mode
    #[arg(short, long, default_value_t = false)]
    reverse: bool,
}

const BUFFER_SIZE: usize = 1024;

#[tokio::main]
async fn main() -> Result<(), Error> {
    let args = Args::parse();

    let server_mode = args.ip_host.is_empty();
    let sending_mode = !(server_mode ^ args.reverse);
    println!(
        "mode: {} {}",
        if server_mode { "server" } else { "client" },
        if sending_mode { "sending" } else { "receiving" }
    );
    println!("port: {}", args.port);
    println!(" dir: {}", args.directory);

    let res = if server_mode {
        server(args, sending_mode).await
    } else {
        client(args, sending_mode).await
    };
    if let Err(error) = res {
        println!("ERROR: {}", error);
        std::process::exit(-1)
    } else {
        Ok(())
    }
}

/*
protocol is :
u64  - relative path name size
utf8 - relative path name
u64  - file size
u8   - file content

receiver listens for a socket connection and gets the protocol file and close connection.
sender iterates through the directory files and send each of them as a separate or same connection.

NB: empty filename indicates end of transfer
*/

async fn client(args: Args, sending_mode: bool) -> Result<(), Error> {
    let path = Path::new(&args.directory);
    let dir = validate_path(path)?;
    let stream = TcpStream::connect(format!("{}:{}", args.ip_host, args.port)).await?;

    println!("----------------------------------------");
    println!(
        "{} directory: {}",
        if sending_mode { "sending" } else { "receiving" },
        dir
    );
    println!("----------------------------------------");
    if sending_mode {
        directory_send(dir, stream).await?;
    } else {
        directory_receive(dir, stream).await?;
    };
    Ok(())
}

async fn server(args: Args, sending_mode: bool) -> Result<(), Error> {
    let path = Path::new(&args.directory);
    let dir = validate_path(path)?;

    let listener = create_listener(args.port).await?;
    println!("----------------------------------------");
    println!("start  listening on: {}", args.port);
    println!(
        "{} directory: {}",
        if sending_mode {
            "sending  "
        } else {
            "receiving"
        },
        dir
    );
    println!("----------------------------------------");

    loop {
        let (stream, addr) = listener_accept_connection(&listener).await?;
        println!("new connection from: {}", addr);
        tokio::spawn(connection_handler(sending_mode, dir.clone(), stream));
    }
}

async fn connection_handler(sending_mode: bool, dir: String, stream: TcpStream) {
    let result = if sending_mode {
        directory_send(dir, stream).await
    } else {
        directory_receive(dir, stream).await
    };
    if let Err(error) = result {
        println!("ERROR: {}", error);
        std::process::exit(-1);
    };
}

fn validate_path(path: &Path) -> Result<String, Error> {
    if path.is_dir() {
        let absolute_path = fs::canonicalize(path)?;
        let res = absolute_path.to_str().unwrap();
        Ok(res.to_string())
    } else {
        Err(Error::msg("Not a directory"))
    }
}

async fn create_listener(port: u16) -> Result<TcpListener, Error> {
    let listener = TcpListener::bind(format!("0.0.0.0:{}", port)).await;
    match listener {
        Ok(listener) => Ok(listener),
        Err(e) => Err(e.into()),
    }
}

async fn listener_accept_connection(
    listener: &TcpListener,
) -> Result<(TcpStream, SocketAddr), Error> {
    let accepted = listener.accept().await;
    match accepted {
        Ok((stream, addr)) => Ok((stream, addr)),
        Err(e) => Err(e.into()),
    }
}

async fn directory_send(dir: String, mut stream: TcpStream) -> Result<(), Error> {
    let prefix_len = dir.len();
    for entry in WalkDir::new(dir).into_iter().filter_map(|e| e.ok()) {
        if entry.path().is_file() {
            let full_file_name = entry.path().display().to_string();
            let relative_file_name = &full_file_name[prefix_len..];
            if !relative_file_name.is_empty() {
                print!("sending: {} ... ", relative_file_name);
                write_file(&full_file_name, relative_file_name, &mut stream).await?;
                println!("DONE");
            }
        }
    }
    // send 0 to indicate end of transfer
    write_u64(&mut stream, 0).await?;
    println!("DONE");
    Ok(())
}

async fn directory_receive(dir: String, mut stream: TcpStream) -> Result<(), Error> {
    loop {
        let has_next = save_file(&dir, &mut stream).await?;
        if !has_next {
            println!("DONE");
            break;
        }
    }
    Ok(())
}

async fn save_file(dir: &String, stream: &mut TcpStream) -> Result<bool, Error> {
    let file_name = read_string(stream).await?;
    if file_name.is_empty() {
        return Ok(false);
    }
    let name = denormalize_name(file_name);
    let mut full_name = dir.to_owned();
    full_name.push_str(&name);
    let mut len = read_u64(stream).await?;
    print!("writing: {}...", full_name);

    {
        // check if folder exists
        let mut path = PathBuf::from(&full_name);
        path.pop();
        if !path.exists() {
            fs::create_dir_all(path)?;
        }
    }
    // open file
    let mut file = std::fs::File::create(&full_name)?;
    // write content
    let mut buffer = [0; BUFFER_SIZE];
    while len > 0 {
        if len > BUFFER_SIZE as u64 {
            stream.read_exact(&mut buffer).await?;
            let _ = file.write(&buffer)?;
            len -= BUFFER_SIZE as u64;
        } else {
            let chunk = read_chunk(stream, len as usize).await?;
            let _ = file.write(&chunk)?;
            len -= chunk.len() as u64;
        }
    }
    println!("DONE");
    Ok(true)
}

async fn read_u64(stream: &mut TcpStream) -> Result<u64, Error> {
    let mut len_bytes = [0; 8];
    stream.read_exact(&mut len_bytes).await?;
    let str_len: u64 = u64::from_be_bytes(len_bytes);
    Ok(str_len)
}

async fn read_chunk(stream: &mut TcpStream, chunk_len: usize) -> Result<Vec<u8>, Error> {
    if chunk_len > 0 {
        let mut frame_data = vec![0; chunk_len];
        stream.read_exact(&mut frame_data).await?;
        Ok(frame_data)
    } else {
        Err(Error::msg("reading zero buffer is not allowed"))
    }
}

async fn read_string(stream: &mut TcpStream) -> Result<String, Error> {
    let str_len = read_u64(stream).await?;
    if str_len == 0 {
        Ok(String::new())
    } else if str_len < 4096 {
        let vec = read_chunk(stream, str_len as usize).await?;
        let str = String::from_utf8(vec)?;
        Ok(str)
    } else {
        Err(Error::msg(format!(
            "string should not be longer than 4096 ({})",
            str_len
        )))
    }
}

async fn write_u64(stream: &mut TcpStream, data: u64) -> Result<(), Error> {
    let len_bytes = data.to_be_bytes();
    stream.write_all(&len_bytes).await?;
    Ok(())
}

pub async fn write_buffer(
    stream: &mut TcpStream,
    data: &[u8; BUFFER_SIZE],
    data_len: u64,
) -> Result<(), Error> {
    if data_len > 0 {
        let len = data_len as usize;
        stream.write_all(&data[..len]).await?;
        Ok(())
    } else {
        Err(Error::msg("writing zero buffer is not allowed"))
    }
}

async fn write_string(stream: &mut TcpStream, message: &str) -> Result<(), Error> {
    let data = message.as_bytes();
    let len = data.len();
    if len > 0 {
        write_u64(stream, len as u64).await?;
        stream.write_all(data).await?;
        Ok(())
    } else {
        Err(Error::msg("writing empty string is not allowed"))
    }
}

async fn write_file(
    file_name: &str,
    remote_name: &str,
    stream: &mut TcpStream,
) -> Result<(), Error> {
    let file = File::open(file_name)?;
    let mut len = file.metadata().unwrap().len();
    let mut reader = BufReader::new(file);
    let normalized_name = normalize_name(remote_name.to_string());
    // write normalized filename
    write_string(stream, &normalized_name).await?;
    // write the length
    write_u64(stream, len).await?;
    // write chunks
    let mut buffer = [0; BUFFER_SIZE];
    while len > 0 {
        let read_bytes = reader.read(&mut buffer)? as u64;
        write_buffer(stream, &buffer, read_bytes).await?;
        len -= read_bytes;
    }
    Ok(())
}

// replace os specific char for path with \0 character
fn normalize_name(name: String) -> String {
    let path_char = std::path::MAIN_SEPARATOR;
    name.replace(path_char, "\0")
}
// replace \0 with os specific char
fn denormalize_name(name: String) -> String {
    let path_char = std::path::MAIN_SEPARATOR.to_string();
    name.replace("\0", &path_char)
}
