# rusty-pixel

Image proxy that applies image transformations to the given image.

## Transformations

### Options

To generate an image with ratio 40/30 and with 10% margin that is resize to 200 height before

`<proxy_base_url>/scale/rh200-s30x40-m10/<url>`

- `GET` `/scale/` - Scale mode
  - `s<a size>x<b size>` - Scale with ratio
  - `r<w|h><pixels>` - Resize image to max `w` (width) or max `h` (height)
  - `m<percentage>` - Add margin from percentage base on original size, this makes the image bigger
  - `o<portrait|landscape>` - Force orientation of the image
  - `bw` - Black and white

> Resize is performed after all options.

Examples

- `s40x30` - **Scale by 40 / 30**
- `rw1000` - **Resize image to width 1000**
- Scale with margin
  - `s40x30-m10` - **Scale by 40 / 30 with added percentage margin of the shortest side**

## Contributing

### Pull Request Process

1. Ensure any install or build dependencies are not in version control.
2. Update the README.md with details of changes to the interface, this includes new environment variables, exposed ports, useful file locations and container
   parameters.
3. You may merge Pull Requests.
4. Delete branch after merge.
