import subprocess
import yaml
import logging

# Set up logging
logging.basicConfig(level=logging.INFO, format='%(asctime)s - %(levelname)s - %(message)s')

# Load configuration
with open('config.yaml', 'r') as config_file:
    config = yaml.safe_load(config_file)

SENDOOK = 'sendook -f 304300000 -0 333 -1 333 '
PAUSE = '0' * 30

def construct_raw_bits_from_bits(bits):
    return ''.join(f'10{bit}' for bit in bits)

def modify_bit_pos(bits, pos, val):
    return bits[:pos] + val + bits[pos + 1:]

def construct_full_raw_cmd(fanid, cmdid):
    preamble = '1111'
    return construct_raw_bits_from_bits(preamble + fanid + modify_bit_pos(cmdid, 2, '1')) + PAUSE

CMDS = {
    'light': '0000000010',
    'stop': '0000000100',
    'reverse': '0000001000',
    'speed1': '0000010000',
    'speed2': '0010000000',
    'speed3': '0000100000',
    'speed4': '0000110000',
    'speed5': '0001000100',
    'speed6': '0001000000'
}

# Fan IDs are now loaded from the config file
FAN_IDS = config['fans']

def control_fan(room_name, cmd):
    logging.info(f"Control fan: {room_name}, {cmd}")
    if room_name not in FAN_IDS:
        raise ValueError(f"Invalid room name: {room_name}")
    if cmd not in CMDS:
        raise ValueError(f"Invalid command: {cmd}")

    full_cmd = SENDOOK + construct_full_raw_cmd(FAN_IDS[room_name], CMDS[cmd])

    # Log the command that's about to be executed
    logging.info(f"Executing command: {full_cmd}")

    try:
        # Run the command, capturing stdout and stderr
        result = subprocess.run(full_cmd, shell=True, check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)

        # Log successful execution
        logging.info(f"Command executed successfully. Return code: {result.returncode}")

        # If there's any output, log it at debug level
        if result.stdout:
            logging.debug(f"Command stdout: {result.stdout}")
        if result.stderr:
            logging.debug(f"Command stderr: {result.stderr}")

    except subprocess.CalledProcessError as e:
        # Log error if the command fails
        logging.error(f"Error executing command. Return code: {e.returncode}")
        if e.stdout:
            logging.error(f"Command stdout: {e.stdout}")
        if e.stderr:
            logging.error(f"Command stderr: {e.stderr}")
    except Exception as e:
        # Log any other exceptions
        logging.error(f"Unexpected error executing command: {str(e)}")

# Example usage:
# control_fan('living_room', 'light')
